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
  Start-Process -FilePath (Join-Path $install "uninstall.exe")
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
