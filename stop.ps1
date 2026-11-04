$ErrorActionPreference = 'Stop'
$binary = Join-Path $PSScriptRoot 'target\debug\rust-rcs-lab.exe'
$pidFile = Join-Path $PSScriptRoot 'run\pids.json'
if (-not (Test-Path -LiteralPath $pidFile)) { Write-Host 'No project process records.'; exit 0 }
foreach ($entry in (Get-Content -LiteralPath $pidFile -Raw | ConvertFrom-Json)) {
    $running = Get-CimInstance Win32_Process -Filter "ProcessId=$($entry.process_id)" -ErrorAction SilentlyContinue
    if ($running -and $running.ExecutablePath -eq $binary -and $running.CommandLine -match "--role\s+$($entry.role)(\s|$)") {
        Stop-Process -Id $entry.process_id
        Write-Host "Stopped $($entry.role)."
    }
}
Write-Host 'Task history is preserved in this project: data\rcs.json.'
