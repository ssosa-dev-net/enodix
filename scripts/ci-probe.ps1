# Temporary (issue 72): what does the runner's desktop allow?
$ErrorActionPreference = 'Continue'
"session id: $([System.Diagnostics.Process]::GetCurrentProcess().SessionId)"
whoami
try { Set-Clipboard -Value 'probe-direct'; "direct set ok, get: $(Get-Clipboard -Raw)" } catch { "direct clipboard failed: $_" }
$edge = Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}' -ErrorAction SilentlyContinue
"webview2: $($edge.pv)"
py -V
"--- on a desktop of its own (run-on-desktop.py)"
py scripts/run-on-desktop.py enodix-probe probe.log powershell -NoProfile -Command "Set-Clipboard -Value 'probe-desktop'; 'desktop set ok, get: ' + (Get-Clipboard -Raw)"
"exit $LASTEXITCODE"
