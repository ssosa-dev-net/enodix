# The installer check on a clean Windows (#139): run by
# scripts/sandbox-inside.ps1 inside Windows Sandbox, never on a computer of
# someone's (it ticks "Delete the application data" and empties the app's
# entries in Credential Manager).
#
#   sandbox-installer.ps1 -Old <older setup.exe> -New <setup.exe>
#
# 1. scripts/check-installer.ps1 as it is (install, upgrade, uninstall, a
#    passive upgrade in another folder; it puts everything back).
# 2. Then what only a throwaway Windows allows: install -New silently, start
#    the installed app and wait for its window, put two saved logins of the
#    app's and one of another program in Credential Manager, upgrade over
#    itself in passive mode as the updater would, then uninstall through the
#    uninstaller's window with "Delete the application data" ticked (driven
#    with UI Automation) and list what is left: the install folder, the
#    uninstall entry, the vendor keys, the shortcuts, the data folders, the
#    app's entries in Credential Manager (the other program's must stay) and
#    the uninstaller's copies in the temp folder.
#
# The exit code is 0 only if check-installer.ps1 passed and nothing of the
# app's is left.

param(
  [Parameter(Mandatory = $true)][string]$Old,
  [Parameter(Mandatory = $true)][string]$New
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$tauri = Get-Content (Join-Path $root "src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json
$product = $tauri.productName
$publisher = $tauri.bundle.publisher
$identifier = $tauri.identifier
$install = Join-Path $env:LOCALAPPDATA $product
$uninstallKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$product"
$vendorKeys = @("HKCU:\Software\$publisher", "HKCU:\Software\$($identifier.Split('.')[1])") | Select-Object -Unique
$dataFolders = @((Join-Path $env:APPDATA $identifier), (Join-Path $env:LOCALAPPDATA $identifier))
$problems = New-Object System.Collections.Generic.List[string]

Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
Add-Type -Namespace Win32 -Name Box -MemberDefinition @'
[DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr window, uint message, IntPtr w, IntPtr l);
[DllImport("user32.dll")] public static extern bool PostMessage(IntPtr window, uint message, IntPtr w, IntPtr l);
'@
$automation = [System.Windows.Automation.AutomationElement]

function Problem([string]$text) { $problems.Add($text); Write-Output "PROBLEM: $text" }

function Step([string]$text) { Write-Output ""; Write-Output "=== $text" }

function Elapsed($started) { [int]((Get-Date) - $started).TotalSeconds }

# A top-level window whose name matches, within $seconds, or $null.
function WaitWindow([string]$like, [int]$seconds) {
  $deadline = (Get-Date).AddSeconds($seconds)
  while ((Get-Date) -lt $deadline) {
    $windows = $automation::RootElement.FindAll([System.Windows.Automation.TreeScope]::Children, [System.Windows.Automation.Condition]::TrueCondition)
    foreach ($window in $windows) { if ($window.Current.Name -like $like) { return $window } }
    Start-Sleep -Milliseconds 300
  }
  return $null
}

# A control of $window by its text (any type, & left out), with a window
# handle of its own, or $null.
function Control($window, [string]$name) {
  $all = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
  $all | Where-Object { ($_.Current.Name -replace '&', '') -eq $name -and $_.Current.NativeWindowHandle -ne 0 -and $_.Current.IsEnabled } | Select-Object -First 1
}

# Clicks a button as Windows does for a click (BM_CLICK, posted: the
# uninstaller's buttons start long work).
function Press($window, [string]$name, [int]$seconds = 20) {
  $deadline = (Get-Date).AddSeconds($seconds)
  while ((Get-Date) -lt $deadline) {
    $button = Control $window $name
    if ($button) {
      [void][Win32.Box]::PostMessage([IntPtr]$button.Current.NativeWindowHandle, 0x00F5, [IntPtr]0, [IntPtr]0)
      return
    }
    Start-Sleep -Milliseconds 300
  }
  throw "no $name button in $($window.Current.Name): $(Texts $window)"
}

function Texts($window) {
  $all = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
  ($all | ForEach-Object { $_.Current.Name } | Where-Object { $_ }) -join " | "
}

function Run([string]$exe, [string]$arguments, [string]$what) {
  $started = Get-Date
  $process = Start-Process -FilePath $exe -ArgumentList $arguments -PassThru -Wait
  Start-Sleep -Seconds 2
  while (Get-Process | Where-Object { $_.ProcessName -match '^(Un|Un_.*|Au_)$' -and $_.StartTime -ge $started }) { Start-Sleep -Milliseconds 500 }
  Write-Output ">>> $what : exit code $($process.ExitCode), $(Elapsed $started) s"
}

function CredentialEntries {
  & cmdkey.exe /list | Where-Object { $_ -match 'Target:' } | ForEach-Object { ($_ -replace '^\s*Target:\s*', '') -replace '^LegacyGeneric:target=', '' }
}

function Leftovers([string]$title) {
  Step $title
  $left = @()
  if (Test-Path $install) {
    $left += "install folder $install : $((Get-ChildItem $install -Recurse -Force | ForEach-Object { $_.FullName.Substring($install.Length + 1) }) -join ', ')"
  }
  if (Test-Path $uninstallKey) { $left += "uninstall entry $uninstallKey" }
  foreach ($key in $vendorKeys) {
    if (Test-Path $key) { $left += "vendor key $key : $((Get-ChildItem $key -Recurse | ForEach-Object Name) -join ', ') $((Get-ItemProperty $key -ErrorAction SilentlyContinue | Out-String) -replace '\s+', ' ')" }
  }
  foreach ($folder in $dataFolders) {
    if (Test-Path $folder) { $left += "data folder $folder : $((Get-ChildItem $folder -Force | ForEach-Object Name) -join ', ')" }
  }
  $links = @()
  $links += Get-ChildItem (Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs") -Recurse -Filter "$product*.lnk" -ErrorAction SilentlyContinue
  $links += Get-ChildItem ([Environment]::GetFolderPath("Desktop")) -Filter "$product*.lnk" -ErrorAction SilentlyContinue
  foreach ($link in $links) { $left += "shortcut $($link.FullName)" }
  foreach ($entry in CredentialEntries) {
    if ($entry -like "$identifier*") { $left += "Credential Manager entry $entry" }
  }
  $temp = @(Get-ChildItem $env:TEMP -Force -ErrorAction SilentlyContinue | Where-Object { $_.Name -match '^~nsu.*\.tmp$|^(Un|Un_.*|Au_)\.exe$' } | ForEach-Object Name)
  if ($temp.Count) { Write-Output "the uninstaller's files left in the temp folder (Windows' own clean-up removes them): $($temp -join ', ')" }
  if ($left.Count) { foreach ($item in $left) { Problem "left after the data-deleting uninstall: $item" } } else { Write-Output "nothing of $product is left" }
}

# --- 1. The installer check of #111 -------------------------------------------

Step "scripts/check-installer.ps1 -Old $Old -New $New"
$started = Get-Date
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root "scripts\check-installer.ps1") -Old $Old -New $New
$checked = $LASTEXITCODE
Write-Output ">>> check-installer.ps1: exit code $checked, $(Elapsed $started) s"
if ($checked -ne 0) { Problem "check-installer.ps1 failed" }

# --- 1b. An upgrade over a version that kept its data in the roaming folder ----
#
# Up to 0.4.1 config backups, kept job results and the Python environment
# were in the roaming data folder; the first start of a later version moves
# them to the local one (#128). Here the older release is installed and
# started once, files are put where it kept them, the new release is
# installed over it and started, and every file must then be in the local
# folder, the same bytes, and gone from the roaming one. What stays in the
# roaming folder (the settings) must still be there.

# Every file below $folder: its path from there and its SHA-256.
function Tree([string]$folder) {
  if (-not (Test-Path $folder)) { return @() }
  @(Get-ChildItem $folder -Recurse -File -Force | Sort-Object FullName | ForEach-Object {
      "$($_.FullName.Substring($folder.Length + 1)) $((Get-FileHash $_.FullName -Algorithm SHA256).Hash)"
    })
}

# Starts the installed app, waits for its window, leaves it $seconds, closes it.
function StartAndClose([string]$what, [int]$seconds) {
  $app = Join-Path $install "$product.exe"
  if (-not (Test-Path $app)) { throw "no $app ($what)" }
  $started = Get-Date
  $process = Start-Process -FilePath $app -PassThru
  $window = WaitWindow "$product*" 60
  if ($window) { Write-Output "$what shows its window after $(Elapsed $started) s" } else { Problem "$what showed no window within 60 s" }
  Start-Sleep -Seconds $seconds
  if (-not $process.HasExited) {
    $process.CloseMainWindow() | Out-Null
    if (-not $process.WaitForExit(10000)) { Stop-Process -Id $process.Id -Force; Write-Output "$what did not close on its window's close: ended" }
  }
  Start-Sleep -Seconds 2
}

try {
  Step "the data of an older version moves at the first start of the new one (#128)"
  $roaming = Join-Path $env:APPDATA $identifier
  $local = Join-Path $env:LOCALAPPDATA $identifier
  Run $Old "/S" "install the older release silently"
  StartAndClose "the older release" 5
  $settingsBefore = @(Get-ChildItem $roaming -Force -ErrorAction SilentlyContinue | ForEach-Object Name)
  Write-Output "the older release left in ${roaming}: $($settingsBefore -join ', ')"

  # What a version up to 0.4.1 kept there, made up: nested folders, a name
  # with a space and one outside ASCII, an empty file, a file of 3 MiB.
  $moved = @("configs", "jobs", "python")
  $seed = @{
    "configs\ws-1\core-sw1\2026-11-01_10-00-00.cfg" = "hostname core-sw1`r`n!`r`nend`r`n"
    "configs\ws-1\core-sw1\2026-11-02_10-00-00.cfg" = "hostname core-sw1`r`ninterface Gi0/1`r`n!`r`nend`r`n"
    "configs\ws-1\edge r2\2026-11-01_10-00-00.cfg"  = "hostname edge-r2`n"
    "configs\ws-1\z$([char]0xFC)rich-fw\latest.cfg"  = "set system host-name zurich`n"
    "jobs\ws-1\2026-11-03_09-00-00\run.yaml"        = "hosts: []`n"
    "jobs\ws-1\2026-11-03_09-00-00\output\r1.txt"   = "r1# show version`n"
    "jobs\ws-1\2026-11-03_09-00-00\output\empty.txt" = ""
    "python\pyvenv.cfg"                             = "home = C:\nowhere`n"
    "python\Lib\site-packages\marker.txt"           = "moved with the rest`n"
  }
  foreach ($path in $seed.Keys) {
    $file = Join-Path $roaming $path
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $file) | Out-Null
    [System.IO.File]::WriteAllText($file, $seed[$path], (New-Object System.Text.UTF8Encoding($false)))
  }
  $big = Join-Path $roaming "jobs\ws-1\2026-11-03_09-00-00\output\big.txt"
  $bytes = New-Object byte[] (3 * 1024 * 1024)
  (New-Object System.Random 128).NextBytes($bytes)
  [System.IO.File]::WriteAllBytes($big, $bytes)

  # And one thing that stays in the roaming folder: a trusted host key.
  $kept = Join-Path $roaming "known_hosts"
  [System.IO.File]::WriteAllText($kept, "[192.0.2.10]:22 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIDkjyJ0Q7mqyl3W7Xk9oQqk0dA4mS8pV0mJbq0dY3x7N`n", (New-Object System.Text.UTF8Encoding($false)))
  $keptHash = (Get-FileHash $kept -Algorithm SHA256).Hash
  $before = @{}
  foreach ($name in $moved) { $before[$name] = Tree (Join-Path $roaming $name) }
  Write-Output "put in the roaming folder: $(($moved | ForEach-Object { "$_ ($($before[$_].Count) files)" }) -join ', ')"

  Run $New "/S" "install the new release over it silently"
  StartAndClose "the new release, first start" 8

  foreach ($name in $moved) {
    $now = Tree (Join-Path $local $name)
    $old = Join-Path $roaming $name
    if (Compare-Object $before[$name] $now) {
      Problem "$name is not whole in $local after the first start: was $($before[$name].Count) files, is $($now.Count); differences: $((Compare-Object $before[$name] $now | ForEach-Object { "$($_.SideIndicator) $($_.InputObject)" }) -join '; ')"
    } elseif (Test-Path $old) {
      Problem "$name is in $local, the same bytes, but still in ${roaming} too: $((Get-ChildItem $old -Recurse -Force | ForEach-Object Name) -join ', ')"
    } else {
      Write-Output "$name moved whole to ${local}: $($now.Count) files, the same bytes, none left in $roaming"
    }
  }
  if (-not (Test-Path $kept)) { Problem "known_hosts is gone from $roaming after the first start" }
  elseif ((Get-FileHash $kept -Algorithm SHA256).Hash -ne $keptHash) { Problem "known_hosts in $roaming changed at the first start" }
  else { Write-Output "known_hosts stayed in $roaming, unchanged" }
  $settingsAfter = @(Get-ChildItem $roaming -Force -ErrorAction SilentlyContinue | ForEach-Object Name)
  $lost = @($settingsBefore | Where-Object { $_ -notin $moved -and $_ -notin $settingsAfter })
  if ($lost.Count) { Problem "gone from $roaming after the first start: $($lost -join ', ')" }
  Write-Output "in $roaming now: $($settingsAfter -join ', ')"
  Write-Output "in $local now: $((Get-ChildItem $local -Force -ErrorAction SilentlyContinue | ForEach-Object Name) -join ', ')"

  # A second start finds nothing to move and changes nothing.
  $settled = @{}
  foreach ($name in $moved) { $settled[$name] = Tree (Join-Path $local $name) }
  StartAndClose "the new release, second start" 5
  foreach ($name in $moved) {
    if (Compare-Object $settled[$name] (Tree (Join-Path $local $name))) { Problem "$name in $local changed at the second start" }
    if (Test-Path (Join-Path $roaming $name)) { Problem "$name is back in $roaming after the second start" }
  }
  Write-Output "a second start changed nothing"
} catch {
  Problem "the data move check stopped: $($_.Exception.Message)"
}

# --- 2. What only a throwaway Windows allows ---------------------------------

try {
  Step "install the new release silently, start the app"
  Run $New "/S" "install silently"
  $app = Join-Path $install "$product.exe"
  if (-not (Test-Path $app)) { throw "no $app after the install" }
  $started = Get-Date
  $process = Start-Process -FilePath $app -PassThru
  $window = WaitWindow "$product*" 60
  if ($window) {
    Write-Output "the installed app shows its window '$($window.Current.Name)' after $(Elapsed $started) s"
    Start-Sleep -Seconds 5
    $engine = @(Get-CimInstance Win32_Process -Filter "Name = 'msedgewebview2.exe'" | Where-Object { $_.ParentProcessId -eq $process.Id })
    Write-Output "its WebView2 browser processes: $($engine.Count)"
  } else {
    Problem "the installed app showed no window within 60 s (running: $(-not $process.HasExited))"
  }
  foreach ($folder in $dataFolders) {
    Write-Output "data folder $folder after the start: $(if (Test-Path $folder) { (Get-ChildItem $folder -Force | ForEach-Object Name) -join ', ' } else { 'none' })"
  }
  if (-not $process.HasExited) {
    $process.CloseMainWindow() | Out-Null
    if (-not $process.WaitForExit(10000)) { Stop-Process -Id $process.Id -Force; Write-Output "the app did not close on its window's close: ended" }
  }

  Step "saved logins in Credential Manager"
  # Test values: names as the app's keychain makes them (KeychainStore,
  # `<identifier>:<scope>:<name>` and `<identifier>.secret:<scope>:<name>`),
  # and one of another program, which must stay.
  & cmdkey.exe /generic:"${identifier}:installer-check:lab" /user:engineer /pass:sandbox-test-only | Out-Null
  & cmdkey.exe /generic:"${identifier}.secret:installer-check:token" /user:token /pass:sandbox-test-only | Out-Null
  & cmdkey.exe /generic:"another-program:keep-me" /user:someone /pass:sandbox-test-only | Out-Null
  Write-Output "entries now: $((CredentialEntries) -join ', ')"

  Step "upgrade over itself in passive mode (as the updater runs the installer)"
  Run $New "/P" "passive upgrade"

  Step "uninstall through its window, with Delete the application data ticked"
  $started = Get-Date
  # The binary's test variable left in the user's environment, naming the
  # other program's prefix: the uninstaller must not pass it on (its logins
  # must stay, checked below).
  $env:ENODIX_KEYCHAIN_PREFIX = "another-program"
  Start-Process -FilePath (Join-Path $install "uninstall.exe")
  Remove-Item Env:ENODIX_KEYCHAIN_PREFIX
  $window = WaitWindow "*Uninstall*" 60
  if (-not $window) { throw "the uninstaller's window did not show" }
  $mainHandle = $window.Current.NativeWindowHandle
  # Tauri's page adds the box with CreateWindowEx (a BUTTON with
  # BS_AUTOCHECKBOX) and reads it with BM_GETCHECK when the page is left:
  # it is found by its text and ticked with BM_SETCHECK.
  $box = $null
  $deadline = (Get-Date).AddSeconds(20)
  while (-not $box -and (Get-Date) -lt $deadline) {
    $all = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
    $box = $all | Where-Object { $_.Current.Name -like "Also delete*" -and $_.Current.NativeWindowHandle -ne 0 } | Select-Object -First 1
    if (-not $box) { Start-Sleep -Milliseconds 300 }
  }
  if (-not $box) { throw "no box on the uninstaller's first page: $(Texts $window)" }
  Write-Output "the box says: $($box.Current.Name)"
  $handle = [IntPtr]$box.Current.NativeWindowHandle
  [void][Win32.Box]::SendMessage($handle, 0x00F1, [IntPtr]1, [IntPtr]0) # BM_SETCHECK
  Write-Output "ticked: $([Win32.Box]::SendMessage($handle, 0x00F0, [IntPtr]0, [IntPtr]0) -eq [IntPtr]1)" # BM_GETCHECK
  Press $window "Uninstall"
  # The keychain's clean-up says so in a message box when it fails.
  $deadline = (Get-Date).AddSeconds(120)
  $finished = $false
  while (-not $finished -and (Get-Date) -lt $deadline) {
    $windows = $automation::RootElement.FindAll([System.Windows.Automation.TreeScope]::Children, [System.Windows.Automation.Condition]::TrueCondition)
    foreach ($other in $windows) {
      try {
        if ($other.Current.ClassName -eq "#32770" -and $other.Current.NativeWindowHandle -ne $mainHandle -and (Control $other "OK")) {
          Problem "the uninstaller showed a message: $(Texts $other)"
          Press $other "OK"
        }
      } catch {}
    }
    try {
      $close = Control $window "Close"
      if ($close) {
        Write-Output "the uninstaller's last page: $(Texts $window)"
        [void][Win32.Box]::PostMessage([IntPtr]$close.Current.NativeWindowHandle, 0x00F5, [IntPtr]0, [IntPtr]0)
        $finished = $true
      }
    } catch { $finished = $true } # the window is gone
    Start-Sleep -Milliseconds 300
  }
  if (-not $finished) { Problem "the uninstaller did not finish within 120 s" }
  while (Get-Process | Where-Object { $_.ProcessName -match '^(Un|Un_.*|Au_)$' }) { Start-Sleep -Milliseconds 500 }
  Write-Output ">>> uninstalled through the window in $(Elapsed $started) s"

  Leftovers "what the uninstall with the data box ticked left"
  $entries = @(CredentialEntries)
  if ($entries -notcontains "another-program:keep-me") { Problem "the other program's Credential Manager entry is gone" }
  else { Write-Output "the other program's Credential Manager entry stayed" }
} catch {
  Problem "the sandbox checks stopped: $_"
}

Step "result"
if ($problems.Count) {
  Write-Output "$($problems.Count) problem(s):"
  foreach ($problem in $problems) { Write-Output "  $problem" }
  exit 1
}
Write-Output "the installer check passed on a clean Windows"
exit 0
