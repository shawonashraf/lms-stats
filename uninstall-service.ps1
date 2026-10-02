# Stops and removes the lms-stats Windows service. The database and log
# under $env:ProgramData\lms-stats are left in place.
#
#   powershell -ExecutionPolicy Bypass -File uninstall-service.ps1
$ErrorActionPreference = "Continue"
$ServiceName = "lms-stats"

function Test-Admin {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    (New-Object Security.Principal.WindowsPrincipal($identity)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

if (-not (Test-Admin)) {
    Write-Host "Requesting administrator rights (UAC)..."
    Start-Process powershell -Verb RunAs -Wait `
        -ArgumentList "-NoProfile -ExecutionPolicy Bypass -File `"$PSCommandPath`""
    exit $?
}

$service = Get-Service $ServiceName -ErrorAction SilentlyContinue
if (-not $service) {
    Write-Host "Service $ServiceName is not installed."
    exit 0
}

if ($service.Status -ne "Stopped") {
    Write-Host "Stopping $ServiceName..."
    sc.exe stop $ServiceName | Out-Null
    $service.WaitForStatus("Stopped", [TimeSpan]::FromSeconds(30))
}

sc.exe delete $ServiceName | Out-Null
if ($LASTEXITCODE -eq 0) {
    Write-Host "Service $ServiceName removed." -ForegroundColor Green
    Write-Host "Database and log kept under $env:ProgramData\lms-stats."
} else {
    Write-Host "sc.exe delete failed with exit code $LASTEXITCODE" -ForegroundColor Red
    exit 1
}
