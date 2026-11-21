# Checks the Windows installer for real, on this computer, as a release's
# step (docs/releasing.md, "Check the installer"):
#
#   npm run test:installer -- -Old <an older release's -setup.exe> [-New <setup.exe>]
#
# -New defaults to this version's installer in target/dist/<version>-unsigned
# (or target/dist/<version>). It stops, changing nothing, if netterm is
# installed for this user. Then, silently, one installer at a time: installs
# -Old, upgrades it to -New while the installed headless binary runs,
# uninstalls while it runs again, installs -New fresh and uninstalls it, and
# upgrades a copy of -Old in another folder in passive mode (as the updater
# runs the installer) and uninstalls it. After each step it reports what is
# left: the install folder, the uninstall entry, the vendor registry keys,
# the shortcuts and the version details; after the fresh install, what the
# installed binary's clear-keychain does with and without --yes on a keychain
# prefix of its own. At the end it puts the computer back as it found it.
#
# It never starts the app, and never uses "Delete the application data": the
# app's data folders are only looked at, and must not change. The headless
# binary it runs is given a data folder of its own and an empty workspace
# (it opens no store and connects to nothing).

param(
  [Parameter(Mandatory = $true)][string]$Old,
  [string]$New = ""
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$tauri = Get-Content (Join-Path $root "src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json
$product = $tauri.productName
$publisher = $tauri.bundle.publisher
$identifier = $tauri.identifier
# Tauri's vendor key before a publisher was set: the identifier's second part.
$oldVendor = $identifier.Split(".")[1]
$cargo = Get-Content (Join-Path $root "Cargo.toml") -Raw
$version = [regex]::Match($cargo, '\[workspace\.package\][^\[]*?\nversion\s*=\s*"([^"]+)"').Groups[1].Value
$wheel = "$product-$version-py3-none-any.whl"

if (-not $New) {
  foreach ($folder in "$version-unsigned", $version) {
    $candidate = Join-Path $root "target\dist\$folder\${product}_${version}_x64-setup.exe"
    if (Test-Path $candidate) { $New = $candidate; break }
  }
  if (-not $New) { throw "no installer for $version in target\dist: build it first (npm run release -- --unsigned)" }
}
foreach ($installer in $Old, $New) {
  if (-not (Test-Path $installer)) { throw "no installer at $installer" }
}

$install = Join-Path $env:LOCALAPPDATA $product
$otherFolder = Join-Path $env:LOCALAPPDATA "$product-installer-check\$product"
$cli = "$product-cli.exe"
$uninstallKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$product"
$vendorKeys = @("HKCU:\Software\$publisher", "HKCU:\Software\$oldVendor") | Select-Object -Unique
$startMenu = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs"
$desktop = [Environment]::GetFolderPath("Desktop")
$dataFolders = @((Join-Path $env:APPDATA $identifier), (Join-Path $env:LOCALAPPDATA $identifier))
$scratch = Join-Path ([IO.Path]::GetTempPath()) "$product-installer-check-$PID"

function Shortcuts {
  $links = @()
  $links += Get-ChildItem $startMenu -Recurse -Filter "$product*.lnk" -ErrorAction SilentlyContinue
  $links += Get-ChildItem $desktop -Filter "$product*.lnk" -ErrorAction SilentlyContinue
  $shell = New-Object -ComObject WScript.Shell
  foreach ($link in $links) {
    [pscustomobject]@{ Path = $link.FullName; Target = $shell.CreateShortcut($link.FullName).TargetPath }
  }
}

function Ours($shortcut) {
  $shortcut.Target -like "$install\*" -or $shortcut.Target -like "$otherFolder\*"
}

function Details([string]$file) {
  $info = (Get-Item $file).VersionInfo
  "$($info.ProductName) | $($info.FileDescription) | $($info.CompanyName) | $($info.LegalCopyright) | $($info.ProductVersion)"
}

# The data folders' top level, to show they do not change.
function DataPicture {
  foreach ($folder in $dataFolders) {
    if (Test-Path $folder) {
      $items = Get-ChildItem $folder -Force | ForEach-Object { "$($_.Name) [$($_.LastWriteTime.ToString('yyyy-MM-dd HH:mm'))]" }
      "data $folder : $($items -join ', ')"
    } else { "data $folder : none" }
  }
}

function Picture([string]$title, [string]$folder = $install) {
  Write-Output ""
  Write-Output "=== $title"
  if (Test-Path $folder) {
    Write-Output "install folder $folder :"
    Get-ChildItem $folder -Recurse -Force | ForEach-Object {
      $relative = $_.FullName.Substring($folder.Length + 1)
      if ($_.PSIsContainer) { "  [dir] $relative" } else { "  $relative  ($($_.Length) bytes)" }
    }
    foreach ($program in "$product.exe", $cli) {
      $path = Join-Path $folder $program
      if (Test-Path $path) { "  version details of ${program}: $(Details $path)" }
    }
  } else { Write-Output "install folder $folder : none" }
  if (Test-Path $uninstallKey) {
    Write-Output "uninstall entry (HKCU):"
    $entry = Get-ItemProperty $uninstallKey
    foreach ($name in "DisplayName", "DisplayVersion", "Publisher", "Comments", "InstallLocation", "UninstallString") {
      if ($null -ne $entry.$name) { "  $name = $($entry.$name)" } else { "  $name : not set" }
    }
  } else { Write-Output "uninstall entry: none" }
  foreach ($key in $vendorKeys) {
    if (Test-Path $key) {
      Write-Output "vendor key $key :"
      Get-ChildItem $key -Recurse | ForEach-Object { "  $($_.Name) : (default) = $((Get-ItemProperty $_.PSPath).'(default)')" }
    } else { Write-Output "vendor key $key : none" }
  }
  $links = @(Shortcuts | Where-Object { Ours $_ })
  if ($links.Count -gt 0) { foreach ($link in $links) { "shortcut $($link.Path) -> $($link.Target)" } }
  else { Write-Output "shortcuts to $product : none" }
  DataPicture
}

function Run([string]$exe, [string]$arguments, [string]$what) {
  $started = Get-Date
  $process = Start-Process -FilePath $exe -ArgumentList $arguments -PassThru -Wait
  # An NSIS uninstaller copies itself to the temp folder, starts the copy
  # and returns at once: wait for the copy (Un.exe; Un_A.exe or Au_.exe in
  # older NSIS) started since then to end.
  Start-Sleep -Seconds 2
  $deadline = (Get-Date).AddSeconds(90)
  while ((Get-Process | Where-Object { $_.ProcessName -match '^(Un|Un_.*|Au_)$' -and $_.StartTime -ge $started }) -and (Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 500
  }
  Start-Sleep -Seconds 1
  Write-Output ""
  Write-Output ">>> $what : exit code $($process.ExitCode), $([int]((Get-Date) - $started).TotalSeconds) s"
}

# The installed headless binary, running as a script would use it: serving
# (no) tunnels for an empty workspace until its standard input closes.
function StartCli([string]$folder) {
  New-Item -ItemType Directory -Force (Join-Path $scratch "workspace"), (Join-Path $scratch "data") | Out-Null
  Set-Content (Join-Path $scratch "workspace\_workspace.yaml") "id: installer-check"
  $start = New-Object System.Diagnostics.ProcessStartInfo
  $start.FileName = Join-Path $folder $cli
  $start.Arguments = "tunnel `"$(Join-Path $scratch 'workspace')`""
  $start.UseShellExecute = $false
  $start.RedirectStandardInput = $true
  $start.RedirectStandardOutput = $true
  $start.CreateNoWindow = $true
  $start.EnvironmentVariables["NETTERM_DATA_DIR"] = Join-Path $scratch "data"
  $start.EnvironmentVariables["NETTERM_KEYCHAIN_PREFIX"] = "netterm-test-installer-check-$PID"
  $process = [System.Diagnostics.Process]::Start($start)
  $line = $process.StandardOutput.ReadLine()
  Write-Output "$cli is running (process $($process.Id)), it said: $line"
  $script:running = $process
}

function StoppedCli([string]$folder) {
  $process = $script:running
  $stopped = $process.WaitForExit(5000)
  Write-Output "the running $cli was closed by the installer: $stopped"
  if (-not $stopped) { $process.Kill() }
  $path = Join-Path $folder $cli
  if (Test-Path $path) {
    Write-Output "$cli in the install folder is now version '$((Get-Item $path).VersionInfo.ProductVersion)'"
  } else { Write-Output "$cli in the install folder: gone" }
}

# The installed headless binary's clear-keychain, as the uninstaller runs it,
# on a keychain prefix of this check's own (never the app's entries): without
# --yes it removes nothing and fails, with it (what the uninstaller passes) it
# succeeds.
function CheckClearKeychain([string]$folder) {
  foreach ($arguments in "clear-keychain", "clear-keychain --yes") {
    $start = New-Object System.Diagnostics.ProcessStartInfo
    $start.FileName = Join-Path $folder $cli
    $start.Arguments = $arguments
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.CreateNoWindow = $true
    $start.EnvironmentVariables["NETTERM_DATA_DIR"] = Join-Path $scratch "data"
    $start.EnvironmentVariables["NETTERM_KEYCHAIN_PREFIX"] = "netterm-test-installer-check-$PID"
    $process = [System.Diagnostics.Process]::Start($start)
    $said = ($process.StandardOutput.ReadToEnd() + $process.StandardError.ReadToEnd()).Trim()
    $process.WaitForExit()
    Write-Output "$cli $arguments (test prefix): exit code $($process.ExitCode), it said: $said"
  }
}

function ResetVendorKeys {
  foreach ($key in $vendorKeys) {
    if ((Test-Path $key) -and -not $keysBefore[$key]) { Remove-Item $key -Recurse -Force }
  }
}

function CheckUpgrade([string]$folder) {
  $wheels = @(Get-ChildItem (Join-Path $folder "python") -Filter "*.whl" -ErrorAction SilentlyContinue | ForEach-Object Name)
  Write-Output "wheels in python\: $($wheels -join ', ') (expected only $wheel): $(($wheels.Count -eq 1) -and ($wheels[0] -eq $wheel))"
  Write-Output "LICENSE beside the program: $(Test-Path (Join-Path $folder 'LICENSE'))"
}

# --- Before anything ---------------------------------------------------------

Write-Output "Checking $New over $Old"
if ((Test-Path $install) -or (Test-Path $uninstallKey) -or (Test-Path (Split-Path -Parent $otherFolder))) {
  Write-Output "$product is installed for this user already ($install or its uninstall entry): stopping, nothing was changed"
  exit 1
}
$keysBefore = @{}
foreach ($key in $vendorKeys) { $keysBefore[$key] = Test-Path $key }
$linksBefore = @(Shortcuts | ForEach-Object Path)
$dataBefore = (DataPicture) -join "`n"
Picture "before anything"

$failed = $false
try {
  # 1. An older release, upgraded while its headless binary runs.
  Run $Old "/S" "install the older release silently"
  Picture "after installing the older release"
  StartCli $install
  Run $New "/S" "upgrade to the new release silently, with $cli running"
  StoppedCli $install
  CheckUpgrade $install
  Picture "after the upgrade"

  # 2. Uninstalled while its headless binary runs.
  StartCli $install
  Run (Join-Path $install "uninstall.exe") "/S" "uninstall silently, with $cli running"
  StoppedCli $install
  Picture "after uninstalling the upgraded copy"

  # 3. A fresh install of the new release.
  Run $New "/S" "install the new release fresh, silently"
  CheckUpgrade $install
  CheckClearKeychain $install
  Run (Join-Path $install "uninstall.exe") "/S" "uninstall silently"
  Picture "after uninstalling the fresh copy"

  # 4. A copy in another folder, upgraded in passive mode (the updater's).
  # The uninstalls above keep the vendor key (the remembered folder): it goes
  # first, as on a computer that never had the new release.
  ResetVendorKeys
  Run $Old "/S /D=$otherFolder" "install the older release silently in $otherFolder"
  Run $New "/P" "upgrade in passive mode"
  Write-Output "the upgrade went to the copy's folder: $(Test-Path (Join-Path $otherFolder 'LICENSE')); a copy in the default folder: $(Test-Path $install)"
  CheckUpgrade $otherFolder
  Picture "after the passive upgrade" $otherFolder
  Run (Join-Path $otherFolder "uninstall.exe") "/S" "uninstall silently"
  Picture "after uninstalling it" $otherFolder
} catch {
  $failed = $true
  Write-Output "the check stopped: $_"
} finally {
  # --- Put the computer back -------------------------------------------------
  Get-Process -Name "$product-cli" -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -like "$install\*" -or $_.Path -like "$otherFolder\*" } | Stop-Process -Force
  foreach ($folder in $install, (Split-Path -Parent $otherFolder), $scratch) {
    if (Test-Path $folder) { Remove-Item $folder -Recurse -Force }
  }
  if (Test-Path $uninstallKey) { Remove-Item $uninstallKey -Recurse -Force }
  ResetVendorKeys
  Shortcuts | Where-Object { (Ours $_) -and ($linksBefore -notcontains $_.Path) } | ForEach-Object { Remove-Item $_.Path -Force }
  Picture "after putting the computer back"
  $dataAfter = (DataPicture) -join "`n"
  Write-Output "the data folders are as they were: $($dataAfter -eq $dataBefore)"
}
if ($failed) { exit 1 }
