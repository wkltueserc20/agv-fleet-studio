param([ValidateSet('build','test')][string]$Action = 'build')
$ErrorActionPreference = 'Stop'
Set-Location -LiteralPath $PSScriptRoot
$env:CARGO_TARGET_DIR = Join-Path $PSScriptRoot 'target'
& cargo $Action --locked
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
if ($Action -eq 'build') { Write-Host (Join-Path $env:CARGO_TARGET_DIR 'debug\rust-rcs-lab.exe') }
