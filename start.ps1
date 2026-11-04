param([switch]$SkipBuild)
$ErrorActionPreference = 'Stop'
$projectDir = $PSScriptRoot
Set-Location -LiteralPath $projectDir
$binary = Join-Path $projectDir 'target\debug\rust-rcs-lab.exe'
if (-not $SkipBuild -or -not (Test-Path -LiteralPath $binary)) {
    & (Join-Path $projectDir 'build.ps1') -Action build
    if ($LASTEXITCODE -ne 0) { throw 'Rust build failed.' }
}
$services=@(
    @{name='agv01';role='agv';port=8212;vehicle_id='CT-SIM-01'},
    @{name='agv02';role='agv';port=8213;vehicle_id='CT-SIM-02'},
    @{name='agv03';role='agv';port=8214;vehicle_id='CT-SIM-03'},
    @{name='rcs';role='rcs';port=8211;vehicle_id=$null},
    @{name='integration-service';role='central';port=8210;vehicle_id=$null},
    @{name='ecs';role='ecs';port=8216;vehicle_id=$null},
    @{name='wcs';role='wcs';port=8215;vehicle_id=$null}
)
foreach ($service in $services) {
    $probe = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, $service.port)
    try { $probe.Start() } catch { throw "Port $($service.port) is in use. Stop this project's services first." } finally { $probe.Stop() }
}
$runDir = Join-Path $projectDir 'run'
New-Item -ItemType Directory -Path $runDir -Force | Out-Null
$started = @()
$env:RCS_DATA_DIR = Join-Path $projectDir 'data'
try {
    foreach ($service in $services) {
        $arguments=@('--role',$service.role)
        if ($service.vehicle_id) { $arguments+=@('--vehicle-id',$service.vehicle_id) }
        $process = Start-Process -FilePath $binary -ArgumentList $arguments -WorkingDirectory $projectDir -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $runDir "$($service.name).log") -RedirectStandardError (Join-Path $runDir "$($service.name).error.log")
        $started += [pscustomobject]@{ role=$service.role; name=$service.name; vehicle_id=$service.vehicle_id; port=$service.port; process_id=$process.Id; binary=$binary; data_dir=$env:RCS_DATA_DIR }
    }
    $started | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $runDir 'pids.json') -Encoding UTF8
    foreach ($entry in $started) {
        $ready = $false
        for ($attempt=0; $attempt -lt 20; $attempt++) {
            try {
                $health=Invoke-RestMethod -Uri "http://127.0.0.1:$($entry.port)/health" -TimeoutSec 1
                if ($health.role -eq $entry.role -and (-not $entry.vehicle_id -or $health.vehicle_id -eq $entry.vehicle_id)) { $ready=$true; break }
            } catch {}
            Start-Sleep -Milliseconds 200
        }
        if (-not $ready) { throw "Service $($entry.name) failed. See run/*.error.log." }
    }
    Write-Host 'Integration: http://127.0.0.1:8210'
    Write-Host 'RCS:     http://127.0.0.1:8211'
    Write-Host 'AGV 01:  http://127.0.0.1:8212'
    Write-Host 'AGV 02:  http://127.0.0.1:8213'
    Write-Host 'AGV 03:  http://127.0.0.1:8214'
    Write-Host 'WCS:     http://127.0.0.1:8215'
    Write-Host 'ECS:     http://127.0.0.1:8216'
} catch {
    foreach ($entry in $started) { Stop-Process -Id $entry.process_id -ErrorAction SilentlyContinue }
    throw
}
