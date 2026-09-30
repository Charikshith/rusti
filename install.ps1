# rusti installer for Windows:
#   irm https://raw.githubusercontent.com/Charikshith/rusti/master/install.ps1 | iex
# Puts rusti.exe in ~/.rusti/bin and adds that folder to your user PATH.
# Run it again to update.
$ErrorActionPreference = 'Stop'
$bin = Join-Path $HOME '.rusti\bin'
$exe = Join-Path $bin 'rusti.exe'
$url = 'https://github.com/Charikshith/rusti/releases/latest/download/rusti-windows-x86_64.exe'

New-Item -ItemType Directory -Force $bin | Out-Null
# A running rusti.exe cannot be overwritten, but it can be renamed.
if (Test-Path $exe) {
    Remove-Item "$exe.old" -Force -ErrorAction SilentlyContinue
    Rename-Item $exe 'rusti.exe.old'
}
Write-Host "downloading $url"
Invoke-WebRequest $url -OutFile $exe -UseBasicParsing
Remove-Item "$exe.old" -Force -ErrorAction SilentlyContinue

$path = [Environment]::GetEnvironmentVariable('Path', 'User')
if (($path -split ';') -notcontains $bin) {
    [Environment]::SetEnvironmentVariable('Path', ($path.TrimEnd(';') + ";$bin").TrimStart(';'), 'User')
    Write-Host "added $bin to your PATH (open a new terminal to pick it up)"
}
$env:Path = "$env:Path;$bin"
Write-Host "installed $(& $exe --version) -> $exe"
Write-Host "run: rusti"
