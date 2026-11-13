$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$repo = (Resolve-Path '.').Path
$mirror = Join-Path $repo 'release-qa/mirror'
$evidence = Join-Path $repo 'release-qa/evidence'
New-Item -ItemType Directory $evidence | Out-Null
$archiveName = "Kiln-0.2.24-windows-$env:KILN_QA_ARCH.zip"
$archive = Join-Path $mirror $archiveName
$length = (Get-Item -LiteralPath $archive).Length
$sha = (Get-FileHash -LiteralPath $archive).Hash.ToLowerInvariant()
"$sha  $archiveName" | Set-Content (Join-Path $mirror 'SHA256SUMS.txt') -Encoding ascii
$server = Start-Process python -ArgumentList "-u release-qa/mirror.py release-qa/mirror" -PassThru -RedirectStandardOutput (Join-Path $evidence 'mirror.out') -RedirectStandardError (Join-Path $evidence 'mirror.log')
function Assert-Exit([string]$what) { if ($LASTEXITCODE -ne 0) { throw "$what exited $LASTEXITCODE" } }
function Snapshot([string]$dir) {
    @(Get-ChildItem -LiteralPath $dir -File -Recurse | Sort-Object FullName | ForEach-Object {
        "$($_.FullName) $($_.LastWriteTimeUtc.Ticks) $((Get-FileHash -LiteralPath $_.FullName).Hash)"
    }) -join "`n"
}
function Try-StartGui([string]$path, [string]$label) {
    $gui = Start-Process -FilePath $path -PassThru
    for ($i = 0; $i -lt 100; $i++) {
        Start-Sleep -Milliseconds 150
        $gui.Refresh()
        if ($gui.HasExited) { throw "$label exited during startup ($($gui.ExitCode))" }
        if ($gui.MainWindowHandle -ne 0) { Write-Host "$label opened a native window"; return $gui }
    }
    Stop-Process -Id $gui.Id -ErrorAction SilentlyContinue
    throw "$label did not open a window"
}
try {
    $portPath = Join-Path $mirror 'port'
    for ($i = 0; $i -lt 60 -and !(Test-Path $portPath); $i++) { Start-Sleep -Milliseconds 100 }
    $base = 'http://127.0.0.1:' + (Get-Content $portPath -Raw)
    $signature = $env:KILN_QA_SIGNATURE
    if ($signature -notmatch '^[A-Za-z0-9+/]{86}==$') { throw 'Expected an Ed25519 signature' }
    @"
<rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle" xmlns:kiln="https://github.com/YangTaeyoung/kiln/appcast"><channel><item><sparkle:version>0.2.24</sparkle:version><enclosure url="$base/releases/download/v0.2.24/$archiveName" length="$length" sparkle:os="windows" kiln:arch="$env:KILN_QA_ARCH" sparkle:edSignature="$signature" /></item></channel></rss>
"@ | Set-Content (Join-Path $mirror 'appcast-windows.xml') -Encoding ascii
    $env:KILN_RELEASE_URL = $base
    $env:KILN_INSTALL_DIR = Join-Path $env:RUNNER_TEMP 'Kiln-Fresh'
    $legacyShell = Join-Path $env:SystemRoot 'System32/WindowsPowerShell/v1.0/powershell.exe'
    & $legacyShell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File install.ps1
    Assert-Exit 'first install.ps1'
    $first = Snapshot $env:KILN_INSTALL_DIR
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    & $legacyShell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File install.ps1
    Assert-Exit 'second install.ps1'
    if ((Snapshot $env:KILN_INSTALL_DIR) -ne $first -or [Environment]::GetEnvironmentVariable('Path', 'User') -ne $userPath) { throw 'Second install changed the installed files or user PATH' }
    $version = & (Join-Path $env:KILN_INSTALL_DIR 'bin/kiln.exe') --version
    if ($version -ne 'kiln 0.2.24') { throw "Wrong candidate CLI version: $version" }
    Write-Host "Installer ran twice; installed $version on $env:KILN_QA_ARCH with identical files and PATH on the second run."

    $prev = Join-Path $env:RUNNER_TEMP 'Kiln-Upgrade'
    $old = Join-Path $env:RUNNER_TEMP 'kiln-previous.zip'
    $releaseUrl = 'https://github.com/YangTaeyoung/kiln/releases/download/v0.2.23'
    curl.exe -fsSL --retry 3 "$releaseUrl/Kiln-0.2.23-windows-$env:KILN_QA_ARCH.zip" -o $old
    Assert-Exit 'download of official 0.2.23'
    Expand-Archive -LiteralPath $old -DestinationPath $prev
    if ((& (Join-Path $prev 'bin/kiln.exe') --version) -ne 'kiln 0.2.23') { throw 'wrong previous version' }
    $source = Join-Path $env:RUNNER_TEMP 'kiln-v023-source'
    git worktree add --detach $source v0.2.23
    Assert-Exit 'v0.2.23 source checkout'
    $fixture = Join-Path $source 'crates/kiln-common/examples'
    New-Item -ItemType Directory -Force $fixture | Out-Null
    Copy-Item release-qa/updater.rs (Join-Path $fixture 'updater.rs')
    @'

[dev-dependencies]
base64 = "=0.22.1"
ring = "=0.17.14"
roxmltree = "=0.21.1"

[features]
updater-test = []
'@ | Add-Content (Join-Path $source 'crates/kiln-common/Cargo.toml')
    rustup show active-toolchain
    if ($LASTEXITCODE -ne 0) {
        rustup toolchain install stable --profile minimal
        Assert-Exit 'rustup'
        rustup default stable
        Assert-Exit 'rustup default'
    }
    Push-Location $source
    try {
        cargo build -p kiln-common --example updater --features updater-test
        Assert-Exit 'compile previous updater source fixture'
    } finally { Pop-Location }
    Move-Item (Join-Path $prev 'Kiln.exe') (Join-Path $prev 'previous-gui.exe')
    Copy-Item (Join-Path $source 'target/debug/examples/updater.exe') (Join-Path $prev 'Kiln.exe')
    $env:KILN_UPDATE_FEED = "$base/appcast-windows.xml"
    Push-Location $prev
    try {
        & .\Kiln.exe
        Assert-Exit '0.2.23 signed update fixture'
    } finally { Pop-Location }
    if ((Snapshot $env:KILN_INSTALL_DIR) -notmatch [regex]::Escape((Get-FileHash (Join-Path $prev 'Kiln.exe')).Hash)) { throw 'Updated GUI does not match the installed archive' }
    $upgradedCli = Join-Path $prev 'bin/kiln.exe'
    if ((& $upgradedCli --version) -ne $version -or (Get-FileHash $upgradedCli).Hash -ne (Get-FileHash (Join-Path $env:KILN_INSTALL_DIR 'bin/kiln.exe')).Hash) { throw 'Updated CLI does not match the installed archive' }
    $env:KILN_CONFIG_DIR = Join-Path $env:RUNNER_TEMP 'kiln-ui-config'
    $env:KILN_SOCKET = '\\.\pipe\kiln-release-verify'
    $window = Try-StartGui (Join-Path $prev 'Kiln.exe') 'Updated Kiln 0.2.24'
    try {
        if ((Test-Path (Join-Path $prev 'Kiln.exe.old')) -or (Test-Path (Join-Path $prev 'bin/kiln.exe.old'))) { throw 'Updated app did not clean up previous executables' }
    } finally {
        if ($window.CloseMainWindow()) { $null = $window.WaitForExit(5000) }
    }
    [ordered]@{ arch = $env:KILN_QA_ARCH; archive_sha256 = $sha; installed = $version; second_install_unchanged = $true; detected_automatically_from = '0.2.23'; checked_release_key = $true; replaced_running_executable = $true; candidate_window_opened = $true } | ConvertTo-Json | Set-Content (Join-Path $evidence 'result.json')
    Get-Content (Join-Path $evidence 'result.json')
} finally {
    Stop-Process -Id $server.Id -ErrorAction SilentlyContinue
}
