# The part of scripts/sandbox.mjs that runs inside Windows Sandbox (#139), as
# its logon command, in the sandbox's own administrator account
# (WDAGUtilityAccount, a local account with no Microsoft account). It reads
# C:\enodix-out\job.json, writes its log to C:\enodix-out\sandbox.log, the
# test's to smoke.log or installer.log, and C:\enodix-out\done.json with the
# exit code last; then it shuts the sandbox down, which ends it.
#
# Smoke mode: the built app, the test server and the tree's files are copied
# from their read-only folders to C:\enodix (the smoke test writes beside
# the app), Node and Python are used where they are mapped, and
# scripts/smoke.mjs runs from C:\enodix with SMOKE_APP_FOLDER,
# SMOKE_TEST_SERVER, SMOKE_PYTHON, SMOKE_SCREENSHOTS, SMOKE_NET_LOGS and
# SMOKE_WEBVIEW2_POLICY set.
#
# Installer mode: scripts/check-installer.ps1 as it is, then the checks only a
# throwaway Windows allows (scripts/sandbox-installer.ps1).

$ErrorActionPreference = "Stop"
$out = "C:\enodix-out"
$log = Join-Path $out "sandbox.log"
$code = 1

function Say([string]$text) {
  $line = "[sandbox $(Get-Date -Format 'HH:mm:ss')] $text"
  Add-Content -Path $log -Value $line -Encoding UTF8
}

# What this Windows has, for the record.
function Facts {
  $os = Get-CimInstance Win32_OperatingSystem
  Say "Windows: $($os.Caption) $($os.Version), user $(whoami), administrator: $(([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator))"
  $webview = "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"
  if (Test-Path $webview) { Say "WebView2 runtime: $((Get-ItemProperty $webview).pv)" } else { Say "WebView2 runtime: not installed" }
  Say "VC++ runtime (vcruntime140.dll in System32): $(Test-Path "$env:SystemRoot\System32\vcruntime140.dll")"
  Say "display: $((Get-CimInstance Win32_VideoController | ForEach-Object { $_.Name }) -join ', ')"
  $addresses = Get-NetIPAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue | Where-Object { $_.IPAddress -notlike '127.*' } | ForEach-Object IPAddress
  Say "network: $(if ($addresses) { $addresses -join ', ' } else { 'none' })"
  $accounts = @(Get-ChildItem "HKCU:\Software\Microsoft\IdentityCRL\StoredIdentities" -ErrorAction SilentlyContinue | ForEach-Object PSChildName)
  Say "Microsoft accounts signed in to Windows: $(if ($accounts.Count) { $accounts.Count } else { 'none' }); OneAuth's folder: $(Test-Path "$env:LOCALAPPDATA\Microsoft\OneAuth")"
  # What may refuse an unsigned program here (the sandbox's own state, not
  # the host's): Smart App Control (0 off, 1 on, 2 evaluating) and the App
  # Control policies enforced.
  $sac = (Get-ItemProperty "HKLM:\SYSTEM\CurrentControlSet\Control\CI\Policy" -ErrorAction SilentlyContinue).VerifiedAndReputablePolicyState
  Say "Smart App Control: $(if ($null -eq $sac) { 'not set' } else { $sac })"
  try {
    $policies = (& CiTool.exe -lp -json | ConvertFrom-Json).Policies | Where-Object { $_.IsEnforced } | ForEach-Object FriendlyName
    Say "App Control policies enforced: $($policies -join '; ')"
  } catch { Say "App Control policies: cannot be listed ($_)" }
}

# Smart App Control turns itself on in the sandbox's Windows (seen on
# 2026-11-22: off at 19:11, on by 23:10) and then refuses every unsigned
# program it has not seen widely, the app's installer and the app among
# them. The sandbox is thrown away after each run, so it is turned off for
# this run only, and the policies reloaded; the host is not touched.
function SmartAppControlOff {
  $key = "HKLM:\SYSTEM\CurrentControlSet\Control\CI\Policy"
  $state = (Get-ItemProperty $key -ErrorAction SilentlyContinue).VerifiedAndReputablePolicyState
  if ($state -eq 0) { return }
  try {
    Set-ItemProperty -Path $key -Name VerifiedAndReputablePolicyState -Value 0 -Type DWord
    & CiTool.exe --refresh --json | Out-Null
    $now = (Get-ItemProperty $key).VerifiedAndReputablePolicyState
    Say "Smart App Control was $state in the sandbox: turned off for this run (now $now)"
  } catch { Say "Smart App Control could not be turned off: $_" }
}

# After a failure: the latest code-integrity events, which name the file a
# policy refused and the policy.
function CodeIntegrity {
  try {
    $events = Get-WinEvent -LogName "Microsoft-Windows-CodeIntegrity/Operational" -MaxEvents 12 -ErrorAction Stop
    foreach ($e in $events) {
      Say "code integrity $($e.Id) at $($e.TimeCreated.ToString('HH:mm:ss')): $(($e.Message -split "`n")[0..2] -join ' ')"
    }
  } catch { Say "code integrity: no events ($_)" }
  try {
    $events = Get-WinEvent -LogName "Microsoft-Windows-AppLocker/EXE and DLL" -MaxEvents 4 -ErrorAction Stop
    foreach ($e in $events) { Say "AppLocker $($e.Id): $(($e.Message -split "`n")[0])" }
  } catch {}
}

function Copy-Into([string]$from, [string]$to) {
  New-Item -ItemType Directory -Force $to | Out-Null
  Copy-Item -Path (Join-Path $from "*") -Destination $to -Recurse -Force
}

function Smoke($job) {
  $f = $job.folders
  $root = "C:\enodix"
  Copy-Into $f.tree $root
  $app = Join-Path $root "app"
  New-Item -ItemType Directory -Force $app | Out-Null
  foreach ($file in "enodix.exe", "enodix-cli.exe") { Copy-Item (Join-Path $f.app $file) $app }
  Copy-Into (Join-Path $f.app "python") (Join-Path $app "python")
  $server = Join-Path $root "server"
  New-Item -ItemType Directory -Force $server | Out-Null
  Copy-Item (Join-Path $f.server "test_ssh_server.exe") $server
  # The test server is a debug build of an example, linked to the VC++
  # runtime, which a clean Windows does not have: it gets the copy the
  # mapped Python ships (only the test's server; the app's programs get
  # nothing).
  if (-not (Test-Path "$env:SystemRoot\System32\vcruntime140.dll")) {
    Copy-Item (Join-Path $f.python "vcruntime140*.dll") $server
    Say "the test server gets vcruntime140.dll from the mapped Python's folder"
  }
  # The test server reads its fixture keys at the path it was built with
  # (the core's tests/support, CARGO_MANIFEST_DIR): they are put there, on a
  # drive letter of its own (subst) if this Windows has no such drive.
  $keys = $job.fixtureKeys
  $drive = Split-Path -Qualifier $keys
  if (-not (Test-Path "$drive\")) {
    $base = "C:\enodix-drive-$($drive.TrimEnd(':'))"
    New-Item -ItemType Directory -Force $base | Out-Null
    & subst.exe $drive $base
  }
  Copy-Into (Join-Path $root "crates\core\tests\fixtures\keys") $keys
  Say "the test server's fixture keys are at $keys, where it was built to read them"
  foreach ($program in (Join-Path $app "enodix-cli.exe"), (Join-Path $server "test_ssh_server.exe")) {
    $started = Start-Process -FilePath $program -ArgumentList "--version" -NoNewWindow -Wait -PassThru -RedirectStandardOutput (Join-Path $out "version.txt") -RedirectStandardError (Join-Path $out "version-error.txt")
    $said = ((Get-Content (Join-Path $out "version.txt") -Raw) + (Get-Content (Join-Path $out "version-error.txt") -Raw)) -replace '\s+', ' '
    Say "$(Split-Path -Leaf $program) --version: exit code $('0x{0:X8}' -f $started.ExitCode) $said"
  }
  Remove-Item (Join-Path $out "version*.txt") -ErrorAction SilentlyContinue
  $env:PATH = "$($f.node);$($f.python);$env:PATH"
  $env:SMOKE_APP_FOLDER = $app
  $env:SMOKE_TEST_SERVER = Join-Path $server "test_ssh_server.exe"
  $env:SMOKE_PYTHON = Join-Path $f.python "python.exe"
  $env:SMOKE_SCREENSHOTS = Join-Path $out "pictures"
  $env:SMOKE_NET_LOGS = Join-Path $out "net-logs"
  # Everything runs elevated here (no UAC), and WebView2 ignores
  # WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS in an elevated process.
  $env:SMOKE_WEBVIEW2_POLICY = "1"
  New-Item -ItemType Directory -Force $env:SMOKE_SCREENSHOTS, $env:SMOKE_NET_LOGS | Out-Null
  Say "node $(& node --version), python $(& python -c 'import sys; print(sys.version.split()[0])'); the smoke test starts"
  $started = Get-Date
  Push-Location $root
  try {
    & cmd.exe /d /c "node scripts\smoke.mjs > `"$out\smoke.log`" 2>&1"
    $result = $LASTEXITCODE
  } finally { Pop-Location }
  Say "the smoke test ended with exit code $result after $([int]((Get-Date) - $started).TotalSeconds) s"
  return $result
}

function Installer($job) {
  $f = $job.folders
  $root = "C:\enodix"
  Copy-Into $f.tree $root
  $installers = Join-Path $root "installers"
  New-Item -ItemType Directory -Force (Join-Path $installers "old"), (Join-Path $installers "new") | Out-Null
  $old = Join-Path $installers "old\$($job.old)"
  $new = Join-Path $installers "new\$($job.new)"
  Copy-Item (Join-Path $f.oldInstaller $job.old) $old
  Copy-Item (Join-Path $f.newInstaller $job.new) $new
  Say "the installer check starts: $($job.new) over $($job.old)"
  $started = Get-Date
  & cmd.exe /d /c "powershell.exe -NoProfile -ExecutionPolicy Bypass -File `"$root\scripts\sandbox-installer.ps1`" -Old `"$old`" -New `"$new`" > `"$out\installer.log`" 2>&1"
  $result = $LASTEXITCODE
  Say "the installer check ended with exit code $result after $([int]((Get-Date) - $started).TotalSeconds) s"
  return $result
}

try {
  Say "logged on; the run starts"
  $job = Get-Content (Join-Path $out "job.json") -Raw | ConvertFrom-Json
  SmartAppControlOff
  Facts
  if ($job.mode -eq "smoke") { $code = Smoke $job } else { $code = Installer $job }
} catch {
  Say "the run stopped: $_"
  $code = 1
} finally {
  if ($code -ne 0) { CodeIntegrity }
  Set-Content -Path (Join-Path $out "done.json") -Value (@{ code = [int]$code } | ConvertTo-Json) -Encoding ASCII
  Say "done (exit code $code); the sandbox shuts down"
  & shutdown.exe /s /t 2 /f
}
