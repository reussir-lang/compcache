$ErrorActionPreference = 'Stop'

# All disk operations target this newly created scratch VHD, never an existing disk.
$id = [Guid]::NewGuid().ToString('N')
$vhd = Join-Path $env:RUNNER_TEMP "compcache-refs-$id.vhdx"
$commands = Join-Path $env:RUNNER_TEMP "compcache-refs-$id.txt"
$drive = @('R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z') |
    Where-Object { -not (Get-PSDrive -Name $_ -ErrorAction SilentlyContinue) } |
    Select-Object -First 1
if (-not $drive) { throw 'No unused drive letter for the ReFS fixture' }
$savedTemp = $env:TEMP
$savedTmp = $env:TMP
$savedExpectedRoot = $env:COMPCACHE_EXPECT_TEMP_ROOT
$savedStress = $env:COMPCACHE_STRESS_SECONDS

try {
    @"
create vdisk file="$vhd" maximum=2048 type=expandable
attach vdisk
create partition primary
format fs=refs unit=4096 quick label=compcache
assign letter=$drive
exit
"@ | Set-Content -Path $commands -Encoding ascii
    & diskpart.exe /s $commands
    if ($LASTEXITCODE -ne 0) { throw "ReFS fixture creation failed: $LASTEXITCODE" }
    $volume = Get-Volume -DriveLetter $drive
    if ($volume.FileSystemType -ne 'ReFS') { throw 'Fixture is not formatted as ReFS' }
    $volume | Format-List DriveLetter, FileSystemType, Size, SizeRemaining

    $testRoot = "${drive}:\compcache-tests"
    New-Item -ItemType Directory -Path $testRoot | Out-Null
    $env:TEMP = $testRoot
    $env:TMP = $testRoot
    $env:COMPCACHE_EXPECT_TEMP_ROOT = $testRoot
    $env:COMPCACHE_STRESS_SECONDS = '10'
    & cargo test --locked --test store --test maintenance --test schema --test process --test platform
    if ($LASTEXITCODE -ne 0) { throw 'ReFS storage tests failed' }
    & cargo test --locked --test stress
    if ($LASTEXITCODE -ne 0) { throw 'ReFS multiprocess stress failed' }
}
finally {
    $env:TEMP = $savedTemp
    $env:TMP = $savedTmp
    $env:COMPCACHE_EXPECT_TEMP_ROOT = $savedExpectedRoot
    $env:COMPCACHE_STRESS_SECONDS = $savedStress
    if (Test-Path $vhd) {
        @"
select vdisk file="$vhd"
detach vdisk
exit
"@ | Set-Content -Path $commands -Encoding ascii
        & diskpart.exe /s $commands
        if ($LASTEXITCODE -ne 0) { throw 'Could not detach the ReFS fixture' }
        Remove-Item -Path $vhd
    }
    Remove-Item -Path $commands -ErrorAction SilentlyContinue
}
