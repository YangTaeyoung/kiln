param([Parameter(Mandatory=$true)][string]$Mirror, [ValidateSet("arm64","x64")][string]$NativeArch="arm64")
$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
$fixture=Join-Path $env:TEMP ('kiln-release-0.2.15-'+[Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixture | Out-Null
$reg=[Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment',$true)
$pathPresent=$reg.GetValueNames() -contains 'Path'
$pathValue=$reg.GetValue('Path',$null,[Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
$pathKind=if($pathPresent){$reg.GetValueKind('Path')}else{$null}
$shortcut=Join-Path ([Environment]::GetFolderPath('Programs')) 'Kiln.lnk'
$shortcutBytes=if(Test-Path $shortcut){[IO.File]::ReadAllBytes($shortcut)}else{$null}
$env:KILN_INSTALL_DIR=Join-Path $fixture 'installed'
$env:KILN_RELEASE_URL=$Mirror
try {
 & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot '..\install.ps1')
 if($LASTEXITCODE -ne 0){throw 'First install failed'}
 $cli=Join-Path $env:KILN_INSTALL_DIR 'bin\kiln.exe'
 if((& $cli --version) -ne 'kiln 0.2.15'){throw 'Installed version mismatch'}
 $before=@(Get-ChildItem $env:KILN_INSTALL_DIR -File -Recurse | ForEach-Object { $_.FullName+'|'+(Get-FileHash $_.FullName -Algorithm SHA256).Hash+'|'+$_.LastWriteTimeUtc.Ticks })
 & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot '..\install.ps1')
 if($LASTEXITCODE -ne 0){throw 'Second install failed'}
 $after=@(Get-ChildItem $env:KILN_INSTALL_DIR -File -Recurse | ForEach-Object { $_.FullName+'|'+(Get-FileHash $_.FullName -Algorithm SHA256).Hash+'|'+$_.LastWriteTimeUtc.Ticks })
 if(Compare-Object $before $after){throw 'Second install changed files'}
 $arches=if($NativeArch -eq 'arm64'){@('arm64','x64')}else{@('x64')}
 foreach($arch in $arches) {
  $zip=Join-Path $fixture ($arch+'.zip');$extract=Join-Path $fixture $arch
  & curl.exe -fLsS --max-time 300 -o $zip "$Mirror/releases/download/v0.2.15/Kiln-0.2.15-windows-$arch.zip"
  if($LASTEXITCODE -ne 0){throw 'Archive download failed'}
  Expand-Archive -LiteralPath $zip -DestinationPath $extract
  $binary=Get-ChildItem $extract -Filter kiln.exe -File -Recurse | Where-Object {$_.Directory.Name -eq 'bin'} | Select-Object -First 1
  if((& $binary.FullName --version) -ne 'kiln 0.2.15'){throw "$arch CLI version mismatch"}
  if($arch -eq $NativeArch) {
   $root=$binary.Directory.Parent.FullName
   foreach($file in Get-ChildItem $root -File -Recurse) {
    $relative=$file.FullName.Substring($root.Length+1);$installed=Join-Path $env:KILN_INSTALL_DIR $relative
    if((Get-FileHash $file.FullName).Hash -ne (Get-FileHash $installed).Hash){throw "Installed file differs: $relative"}
   }
  }
  Write-Output "$arch version PASS; SHA256 $((Get-FileHash $zip).Hash)"
 }
 $env:KILN_CONFIG_DIR=Join-Path $fixture 'config';$env:KILN_SOCKET='kiln-release-'+[Guid]::NewGuid().ToString('N');$env:KILN_NO_AUTO_UPGRADE='1'
 try {
  $sid=(& $cli new --cwd $fixture --name release-smoke -- cmd.exe /K 'echo KILN_RELEASE_SMOKE_OK').Trim()
  $deadline=(Get-Date).AddSeconds(20)
  do {$text=(& $cli read $sid) -join "`n";if($text -match 'KILN_RELEASE_SMOKE_OK'){break};Start-Sleep -Milliseconds 100}while((Get-Date) -lt $deadline)
  if($text -notmatch 'KILN_RELEASE_SMOKE_OK'){throw "Isolated PTY smoke failed: $text"}
 } finally {& $cli shutdown-daemon}
 Write-Output "PASS: native $NativeArch installation, matching files, idempotent reinstall, executable versions, isolated terminal smoke"
} finally {
 if($pathPresent){$reg.SetValue('Path',$pathValue,$pathKind)}else{$reg.DeleteValue('Path',$false)}
 $reg.Close()
 if($null -ne $shortcutBytes){[IO.File]::WriteAllBytes($shortcut,$shortcutBytes)}elseif(Test-Path $shortcut){Remove-Item -LiteralPath $shortcut}
 Write-Output "Fixture retained: $fixture; user Path and shortcut restored"
}
