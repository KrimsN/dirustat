# Builds a throw-away NTFS VHDX, fills it with a synthetic 1M-node fixture and times the MFT scan.
# Run from an ELEVATED PowerShell in the repository root:
#   .\scripts\bench-mft-vhdx.ps1
#   .\scripts\bench-mft-vhdx.ps1 -Cleanup     # detach and delete the VHDX afterwards
# Touches only the VHDX file it creates (default: <repo>\fixtures-local\mft-bench.vhdx, git-ignored).
param(
    [string]$Vhdx = (Join-Path (Get-Location) 'fixtures-local\mft-bench.vhdx'),
    [int]$SizeGB = 16,
    [string]$Letter = 'Z',
    [string]$Size = '1m',
    [int]$Runs = 3,
    [switch]$Cleanup
)
$ErrorActionPreference = 'Stop'

$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run this script from an elevated PowerShell (the MFT path needs administrator rights).'
}
if (Test-Path "${Letter}:\") { throw "Drive letter ${Letter}: is already in use; pass -Letter with a free one." }

function Invoke-Diskpart([string]$Script) {
    $tmp = New-TemporaryFile
    Set-Content -Path $tmp -Value $Script -Encoding ascii
    try { diskpart /s $tmp | Out-Host } finally { Remove-Item $tmp -Force }
}

New-Item -ItemType Directory -Force -Path (Split-Path $Vhdx) | Out-Null
if (Test-Path $Vhdx) { throw "$Vhdx already exists; remove it yourself or pass -Vhdx." }

# Build everything first so compilation never overlaps with measurements.
cargo build --release -p xtask -p dirustat-scan --example mft_bench
if ($LASTEXITCODE -ne 0) { throw 'build failed' }

$mb = $SizeGB * 1024
Invoke-Diskpart @"
create vdisk file="$Vhdx" maximum=$mb type=expandable
select vdisk file="$Vhdx"
attach vdisk
create partition primary
format fs=ntfs quick label=DIRUSTAT_BENCH
assign letter=$Letter
"@

try {
    Write-Host "== generating $Size fixture on ${Letter}: (slow: file creation bound)"
    $gen = Measure-Command { cargo run --release -p xtask -- fixtures --size $Size --out "${Letter}:\fixture" }
    Write-Host ("fixture generated in {0:N0} s" -f $gen.TotalSeconds)

    Write-Host "== scanning ${Letter}:\ ($Runs runs, 'auto' = MFT with fallback, 'portable' = directory walk)"
    cargo run --release -p dirustat-scan --example mft_bench -- "${Letter}:\" $Runs
}
finally {
    if ($Cleanup) {
        Invoke-Diskpart "select vdisk file=`"$Vhdx`"`ndetach vdisk"
        Remove-Item $Vhdx -Force
        Write-Host 'VHDX detached and deleted.'
    } else {
        Write-Host "VHDX left mounted at ${Letter}: ($Vhdx). Re-run with -Cleanup semantics manually: diskpart detach vdisk, then delete the file."
    }
}
