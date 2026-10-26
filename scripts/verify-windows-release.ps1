# Adapted from the validated 0.2.15 installer/PTY verification. Run with pwsh
# on an owned native Windows VM. Never install into an existing user app folder.
param(
    [Parameter(Mandatory=$true)][string]$Mirror,
    [ValidatePattern('^\d+\.\d+\.\d+$')][string]$Version = '0.2.16',
    [ValidateSet('arm64','x64')][string]$NativeArch = 'arm64',
    [Parameter(Mandatory=$true)][ValidatePattern('^[0-9a-fA-F]{64}$')][string]$ExpectedX64Sha256,
    [Parameter(Mandatory=$true)][ValidatePattern('^[0-9a-fA-F]{64}$')][string]$ExpectedArm64Sha256
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) { throw 'A native Windows VM is required' }
$osArch = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
if ($osArch -ne $NativeArch) { throw "Expected native $NativeArch Windows, got $osArch" }
$Mirror = $Mirror.TrimEnd('/')
if ($Mirror -match '/releases/tag/v(?<tag>\d+\.\d+\.\d+)$') {
    if ($Matches.tag -ne $Version) { throw 'Public release URL version mismatch' }
    $Mirror = $Mirror.Substring(0, $Mirror.LastIndexOf('/releases/tag/v'))
}
if ($Mirror -notmatch '^https?://') { throw 'Mirror must be an HTTP(S) repository base or release URL' }

function Fetch([string]$Url, [string]$Destination) {
    & curl.exe -fLsS --retry 3 --connect-timeout 10 --max-time 300 -o $Destination $Url
    if ($LASTEXITCODE -ne 0) { throw "Download failed: $Url" }
}
function FileManifest([string]$Root, [bool]$WithTimes = $false, [bool]$Diagnostics = $false) {
    Get-ChildItem -LiteralPath $Root -File -Recurse | Sort-Object FullName | ForEach-Object {
        $relative = $_.FullName.Substring($Root.Length + 1)
        $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
        if ($Diagnostics) {
            Write-Host "MANIFEST root=[$Root] root-length=$($Root.Length) full=[$($_.FullName)] relative=[$relative] length=$($_.Length) sha256=$hash attributes=$($_.Attributes)"
        }
        $value = $relative + '|' + $_.Length + '|' + $hash
        if ($WithTimes) { $value += '|' + $_.LastWriteTimeUtc.Ticks }
        $value
    }
}
function AssertManifest([string[]]$Expected, [string[]]$Actual, [string]$Message) {
    $difference = @(Compare-Object $Expected $Actual)
    if ($difference.Count) {
        Write-Host "MANIFEST MISMATCH: $Message"
        $Expected | ForEach-Object { Write-Host "EXPECTED $_" }
        $Actual | ForEach-Object { Write-Host "ACTUAL   $_" }
        Write-Host ($difference | Format-Table -AutoSize -Wrap | Out-String)
        throw $Message
    }
}
function AssertPeMachine([string]$File, [string]$Arch) {
    $bytes = [IO.File]::ReadAllBytes($File)
    if ($bytes.Length -lt 64 -or $bytes[0] -ne 0x4d -or $bytes[1] -ne 0x5a) { throw "Not a PE executable: $File" }
    $offset = [BitConverter]::ToInt32($bytes, 0x3c)
    if ($offset -lt 0 -or $offset + 6 -gt $bytes.Length -or [BitConverter]::ToUInt32($bytes, $offset) -ne 0x4550) { throw "Invalid PE header: $File" }
    $expected = if ($Arch -eq 'arm64') { 0xaa64 } else { 0x8664 }
    if ([BitConverter]::ToUInt16($bytes, $offset + 4) -ne $expected) { throw "Wrong $Arch machine type: $File" }
}
function AssertCliVersion([string]$File) {
    $reported = (& $File --version) -join "`n"
    if ($LASTEXITCODE -ne 0 -or $reported.Trim() -ne "kiln $Version") { throw "CLI version mismatch: $File" }
}

$fixture = Join-Path $env:TEMP ("kiln-release-$Version-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixture | Out-Null
$savedEnv = @{}
foreach ($name in @('KILN_INSTALL_DIR','KILN_RELEASE_URL','KILN_CONFIG_DIR','KILN_SOCKET','KILN_ACCOUNTS_SANDBOX','KILN_NO_AUTO_UPGRADE','KILN_DB_NO_KEYCHAIN')) {
    $savedEnv[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
$reg = $null
$shortcut = $null
$shortcutBytes = $null
$shortcutTime = $null
$cli = $null
$smokeStarted = $false
$installerStarted = $false
$isolatedSocket = 'kiln-release-' + [Guid]::NewGuid().ToString('N')
try {
    $env:KILN_INSTALL_DIR = Join-Path $fixture 'installed'
    $env:KILN_RELEASE_URL = $Mirror
    $env:KILN_CONFIG_DIR = Join-Path $fixture 'config'
    $env:KILN_SOCKET = $isolatedSocket
    $env:KILN_ACCOUNTS_SANDBOX = Join-Path $fixture 'accounts'
    $env:KILN_NO_AUTO_UPGRADE = '1'
    $env:KILN_DB_NO_KEYCHAIN = '1'

    # Validate expected bytes before invoking any extracted or installed CLI.
    $nativeRoot = $null
    $arches = if ($NativeArch -eq 'arm64') { @('arm64','x64') } else { @('x64') }
    foreach ($arch in $arches) {
        $zip = Join-Path $fixture ($arch + '.zip')
        $extract = Join-Path $fixture $arch
        Fetch "$Mirror/releases/download/v$Version/Kiln-$Version-windows-$arch.zip" $zip
        $expected = if ($arch -eq 'x64') { $ExpectedX64Sha256 } else { $ExpectedArm64Sha256 }
        $actual = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash
        if ($actual -ne $expected) { throw "Downloaded $arch ZIP differs from final local digest" }
        Expand-Archive -LiteralPath $zip -DestinationPath $extract
        $binary = Join-Path $extract 'bin\kiln.exe'
        AssertPeMachine $binary $arch
        AssertPeMachine (Join-Path $extract 'Kiln.exe') $arch
        AssertCliVersion $binary
        if ($arch -eq $NativeArch) { $nativeRoot = $extract }
        Write-Output "$arch CLI execution/version and PE machine PASS; SHA256 $($actual.ToLowerInvariant())"
    }

    # Capture exact user values/types and shortcut bytes before the real installer.
    $reg = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
    $pathPresent = $reg.GetValueNames() -contains 'Path'
    $pathValue = $reg.GetValue('Path', $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    $pathKind = if ($pathPresent) { $reg.GetValueKind('Path') } else { $null }
    $shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'Kiln.lnk'
    if (Test-Path -LiteralPath $shortcut) {
        $shortcutBytes = [IO.File]::ReadAllBytes($shortcut)
        $shortcutTime = [IO.File]::GetLastWriteTimeUtc($shortcut)
    }
    $installer = Join-Path $PSScriptRoot '..\install.ps1'
    $installerStarted = $true
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $installer
    if ($LASTEXITCODE -ne 0) { throw 'First install failed' }
    $cli = Join-Path $env:KILN_INSTALL_DIR 'bin\kiln.exe'
    AssertCliVersion $cli
    $expectedFiles = @(FileManifest $nativeRoot $false $true)
    $installedFiles = @(FileManifest $env:KILN_INSTALL_DIR $false $true)
    AssertManifest $expectedFiles $installedFiles 'Installed files differ from the exact final archive'
    $before = @(FileManifest $env:KILN_INSTALL_DIR $true)
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $installer
    if ($LASTEXITCODE -ne 0) { throw 'Second install failed' }
    $after = @(FileManifest $env:KILN_INSTALL_DIR $true)
    AssertManifest $before $after 'Second install changed file set, bytes, lengths, hashes, or mtimes'
    AssertCliVersion $cli

    try {
        $smokeStarted = $true
        $sid = ((& $cli new --cwd $fixture --name release-smoke -- cmd.exe /K 'echo KILN_RELEASE_SMOKE_OK>pty-witness.txt & type pty-witness.txt') -join '').Trim()
        if ($LASTEXITCODE -ne 0 -or $sid -notmatch '^[0-9]+$') { throw 'Isolated session creation failed' }
        $deadline = (Get-Date).AddSeconds(20)
        do {
            $text = (& $cli read $sid) -join "`n"
            if ($LASTEXITCODE -ne 0) { throw 'Isolated terminal read failed' }
            $witness = Join-Path $fixture 'pty-witness.txt'
            if ($text -match 'KILN_RELEASE_SMOKE_OK' -and (Test-Path -LiteralPath $witness)) { break }
            Start-Sleep -Milliseconds 100
        } while ((Get-Date) -lt $deadline)
        if ($text -notmatch 'KILN_RELEASE_SMOKE_OK' -or !(Test-Path -LiteralPath $witness) -or (Get-Content -LiteralPath $witness -Raw).Trim() -ne 'KILN_RELEASE_SMOKE_OK') {
            throw 'Isolated PTY output/execution witness failed'
        }
    } finally {
        if ($smokeStarted -and $env:KILN_SOCKET -eq $isolatedSocket) {
            & $cli shutdown-daemon
            if ($LASTEXITCODE -ne 0) { throw 'Isolated daemon shutdown failed' }
            $smokeStarted = $false
        }
    }
} finally {
    try {
        if ($null -ne $reg) {
            try {
                if ($installerStarted) {
                    if ($pathPresent) { $reg.SetValue('Path', $pathValue, $pathKind) } else { $reg.DeleteValue('Path', $false) }
                    $restored = $reg.GetValue('Path', $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
                    if ($pathPresent) {
                        if ($restored -cne $pathValue -or $reg.GetValueKind('Path') -ne $pathKind) { throw 'User Path restoration failed' }
                    } elseif ($reg.GetValueNames() -contains 'Path') { throw 'User Path absence was not restored' }
                }
            } finally { $reg.Dispose() }
        }
    } finally {
        try {
            if ($installerStarted -and $null -ne $shortcut) {
                if ($null -ne $shortcutBytes) {
                    [IO.File]::WriteAllBytes($shortcut, $shortcutBytes)
                    [IO.File]::SetLastWriteTimeUtc($shortcut, $shortcutTime)
                    if ([Convert]::ToBase64String([IO.File]::ReadAllBytes($shortcut)) -cne [Convert]::ToBase64String($shortcutBytes)) { throw 'Shortcut restoration failed' }
                } elseif (Test-Path -LiteralPath $shortcut) { Remove-Item -LiteralPath $shortcut }
            }
        } finally {
            foreach ($name in $savedEnv.Keys) { [Environment]::SetEnvironmentVariable($name, $savedEnv[$name], 'Process') }
            Write-Output "Fixture retained: $fixture; process environment restored"
        }
    }
}
Write-Output "PASS: native $NativeArch $Version installation, final archive bytes, idempotent reinstall, CLI versions, isolated PTY and fixture restoration"
