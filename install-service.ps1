# Installs lms-stats as a Windows service (the equivalent of
# lms-stats.service on Linux). Run from an elevated PowerShell, or let the
# script raise its own UAC prompt:
#
#   powershell -ExecutionPolicy Bypass -File install-service.ps1
#
# Optional parameters:
#   -Upstream "http://127.0.0.1:1234,http://192.168.0.163:1234"
#   -Listen   "0.0.0.0:1235"
#   -DbPath   "C:\ProgramData\lms-stats\lms-stats.db"
param(
    [string]$Upstream = "http://127.0.0.1:1234",
    [string]$Listen = "0.0.0.0:1235",
    [string]$DbPath = ""
)

$ErrorActionPreference = "Stop"
$ServiceName = "lms-stats"

function Test-Admin {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    (New-Object Security.Principal.WindowsPrincipal($identity)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

if (-not (Test-Admin)) {
    Write-Host "Requesting administrator rights (UAC)..."
    $args_ = "-NoProfile -ExecutionPolicy Bypass -File `"$PSCommandPath`" -Upstream `"$Upstream`" -Listen `"$Listen`""
    if ($DbPath) { $args_ += " -DbPath `"$DbPath`"" }
    Start-Process powershell -Verb RunAs -ArgumentList $args_ -Wait
    exit $?
}

if (-not $DbPath) {
    $DbPath = Join-Path $env:ProgramData "lms-stats\lms-stats.db"
}

$exe = Join-Path $PSScriptRoot "target\release\lms-stats.exe"
if (-not (Test-Path $exe)) {
    Write-Host "Release binary not found at $exe" -ForegroundColor Red
    Write-Host "Build it first:  cargo build --release"
    exit 1
}

if (Get-Service $ServiceName -ErrorAction SilentlyContinue) {
    Write-Host "Removing existing $ServiceName service..."
    sc.exe stop $ServiceName | Out-Null
    (Get-Service $ServiceName).WaitForStatus("Stopped", [TimeSpan]::FromSeconds(30))
    sc.exe delete $ServiceName | Out-Null
    Start-Sleep -Seconds 2
}

Write-Host "Creating $ServiceName service (binary: $exe)"
sc.exe create $ServiceName binPath= "`"$exe`" run-as-service" start= auto | Out-Null
if ($LASTEXITCODE -ne 0) {
    Write-Host "sc.exe create failed with exit code $LASTEXITCODE" -ForegroundColor Red
    exit 1
}

sc.exe description $ServiceName "lms-stats: token-counting proxy for LM Studio" | Out-Null

# Per-service environment, read by the service process instead of a shell
# profile. This is the Windows counterpart of the unit's Environment= lines.
New-ItemProperty -Path "HKLM:\SYSTEM\CurrentControlSet\Services\$ServiceName" `
    -Name Environment -PropertyType MultiString -Force `
    -Value @("LMS_UPSTREAM=$Upstream", "LMS_LISTEN=$Listen", "LMS_DB=$DbPath") | Out-Null

# Restart on crash, like Restart=always / RestartSec=3 in the unit file.
sc.exe failure $ServiceName reset= 86400 actions= restart/3000/restart/10000/restart/30000 | Out-Null

sc.exe start $ServiceName | Out-Null
(Get-Service $ServiceName).WaitForStatus("Running", [TimeSpan]::FromSeconds(30))

Write-Host ""
Write-Host "Service $ServiceName is running." -ForegroundColor Green
Write-Host "  Upstream : $Upstream"
Write-Host "  Listening: $Listen"
Write-Host "  Database : $DbPath"
Write-Host "  Log      : $(Join-Path $env:ProgramData 'lms-stats\service.log')"
Write-Host "  Dashboard: http://localhost:1235/dashboard"
