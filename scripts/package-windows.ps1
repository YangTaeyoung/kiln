# Builds Kiln for one Windows target and packs dist\Kiln-<version>-windows-<arch>.zip with
# Kiln.exe, bin\kiln.exe and the license texts, then prints its SHA-256 line.
param(
    [Parameter(Mandatory)][ValidateSet('x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc')][string]$Target,
    [string]$OutDir = 'dist'
)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path -Parent $PSScriptRoot)
$version = ((cargo metadata --locked --no-deps --format-version 1 | ConvertFrom-Json).packages | Where-Object name -eq 'kiln').version
$arch = @{ 'x86_64-pc-windows-msvc' = 'x64'; 'aarch64-pc-windows-msvc' = 'arm64' }[$Target]
cargo build --locked --release -p kiln --bins --target $Target
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
$name = "Kiln-$version-windows-$arch"
$stage = Join-Path $OutDir $name
Remove-Item -Recurse -Force -LiteralPath $stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path (Join-Path $stage 'bin') | Out-Null
Copy-Item "target\$Target\release\kiln-app.exe" (Join-Path $stage 'Kiln.exe')
Copy-Item "target\$Target\release\kiln.exe" (Join-Path $stage 'bin\kiln.exe')
Copy-Item LICENSE (Join-Path $stage 'LICENSE.txt')
python scripts/collect-licenses.py (Join-Path $stage 'THIRD_PARTY_LICENSES.txt')
if ($LASTEXITCODE -ne 0) { throw 'license collection failed' }
$reported = & (Join-Path $stage 'bin\kiln.exe') --version
if ("$reported" -notmatch [regex]::Escape($version)) { throw "bin\kiln.exe reports '$reported', not $version" }
$zip = Join-Path $OutDir "$name.zip"
Remove-Item -Force -LiteralPath $zip -ErrorAction SilentlyContinue
# tar.exe writes '/' separators; the updater extracts with the same tool.
& "$env:SystemRoot\System32\tar.exe" -a -c -f $zip -C $stage Kiln.exe bin LICENSE.txt THIRD_PARTY_LICENSES.txt
if ($LASTEXITCODE -ne 0) { throw 'tar failed' }
"$((Get-FileHash -Algorithm SHA256 -LiteralPath $zip).Hash.ToLowerInvariant())  $name.zip"
