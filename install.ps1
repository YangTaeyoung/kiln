# Install the latest Kiln release for Windows (x64 or ARM64). Existing installations update in Kiln.
# The script is ASCII so Windows PowerShell reads it the same in any code page.
# irm https://raw.githubusercontent.com/YangTaeyoung/kiln/main/install.ps1 | iex
# The body runs in its own scope, so piping it to Invoke-Expression leaves the shell's settings alone.
& {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'

    function Fail([string]$message) { throw "Kiln: $message" }

    # Native helpers: the machine Windows runs on, the Start-menu shortcut and the environment change notice.
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class KilnInstall {
    [DllImport("kernel32.dll")] static extern IntPtr GetCurrentProcess();
    [DllImport("kernel32.dll")] static extern bool IsWow64Process2(IntPtr process, out ushort processMachine, out ushort nativeMachine);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    static extern IntPtr SendMessageTimeout(IntPtr window, uint message, UIntPtr wParam, string lParam, uint flags, uint timeout, out UIntPtr result);
    [ComImport, Guid("000214F9-0000-0000-C000-000000000046"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IShellLinkW {
        void GetPath(IntPtr a, int b, IntPtr c, int d); void GetIDList(out IntPtr a); void SetIDList(IntPtr a);
        void GetDescription(IntPtr a, int b); void SetDescription([MarshalAs(UnmanagedType.LPWStr)] string a);
        void GetWorkingDirectory(IntPtr a, int b); void SetWorkingDirectory([MarshalAs(UnmanagedType.LPWStr)] string a);
        void GetArguments(IntPtr a, int b); void SetArguments([MarshalAs(UnmanagedType.LPWStr)] string a);
        void GetHotkey(out short a); void SetHotkey(short a); void GetShowCmd(out int a); void SetShowCmd(int a);
        void GetIconLocation(IntPtr a, int b, out int c); void SetIconLocation([MarshalAs(UnmanagedType.LPWStr)] string a, int b);
        void SetRelativePath([MarshalAs(UnmanagedType.LPWStr)] string a, int b); void Resolve(IntPtr a, int b);
        void SetPath([MarshalAs(UnmanagedType.LPWStr)] string a);
    }
    [ComImport, Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IPropertyStore {
        void GetCount(out uint a); void GetAt(uint a, out PropertyKey b); void GetValue(ref PropertyKey a, out PropVariant b);
        void SetValue(ref PropertyKey a, ref PropVariant b); void Commit();
    }
    [ComImport, Guid("0000010B-0000-0000-C000-000000000046"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IPersistFile {
        void GetClassID(out Guid a); [PreserveSig] int IsDirty(); void Load([MarshalAs(UnmanagedType.LPWStr)] string a, int b);
        void Save([MarshalAs(UnmanagedType.LPWStr)] string a, bool b); void SaveCompleted([MarshalAs(UnmanagedType.LPWStr)] string a);
        void GetCurFile([MarshalAs(UnmanagedType.LPWStr)] out string a);
    }
    [StructLayout(LayoutKind.Sequential, Pack = 4)]
    struct PropertyKey { public Guid fmtid; public uint pid; }
    [StructLayout(LayoutKind.Explicit)]
    struct PropVariant { [FieldOffset(0)] public ushort vt; [FieldOffset(8)] public IntPtr value; }
    [ComImport, Guid("00021401-0000-0000-C000-000000000046")] class ShellLink { }
    // The machine type Windows runs on (0xAA64 ARM64, 0x8664 x64), also inside an emulated process; 0 when unknown.
    public static ushort NativeMachine() {
        ushort process, native;
        try { return IsWow64Process2(GetCurrentProcess(), out process, out native) ? native : (ushort)0; }
        catch (EntryPointNotFoundException) { return 0; }
    }
    // Tells running programs, Explorer included, that the user environment changed.
    public static void AnnounceEnvironment() {
        UIntPtr result;
        SendMessageTimeout(new IntPtr(0xffff), 0x001A, UIntPtr.Zero, "Environment", 0x0002, 5000, out result);
    }
    public static void CreateShortcut(string path, string target, string folder, string appId) {
        var link = (IShellLinkW)new ShellLink();
        link.SetPath(target);
        link.SetWorkingDirectory(folder);
        link.SetIconLocation(target, 0);
        var key = new PropertyKey { fmtid = new Guid("9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3"), pid = 5 };
        var value = new PropVariant { vt = 31, value = Marshal.StringToCoTaskMemUni(appId) };
        try { var store = (IPropertyStore)link; store.SetValue(ref key, ref value); store.Commit(); }
        finally { Marshal.FreeCoTaskMem(value.value); }
        ((IPersistFile)link).Save(path, true);
    }
}
'@

    $machine = switch ([KilnInstall]::NativeMachine()) {
        0xAA64 { 'ARM64' }
        0x8664 { 'AMD64' }
        default { if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE } }
    }
    $arch = switch ($machine) { 'AMD64' { 'x64' } 'ARM64' { 'arm64' } default { Fail "Kiln supports x64 and ARM64 Windows, not $machine." } }
    $installDir = if ($env:KILN_INSTALL_DIR) { $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($env:KILN_INSTALL_DIR) } else { Join-Path $env:LOCALAPPDATA 'Programs\Kiln' }
    if (Test-Path -LiteralPath (Join-Path $installDir 'Kiln.exe')) {
        Write-Host 'Kiln is already installed. Open Kiln, then Settings > About > Check for Updates to update safely.'
        return
    }

    $curl = Join-Path $env:SystemRoot 'System32\curl.exe'
    if (-not (Test-Path -LiteralPath $curl)) { Fail 'curl.exe is missing; Kiln needs Windows 10 version 1803 or later.' }
    function Fetch([string[]]$arguments) {
        & $curl -fLsS --retry 3 --connect-timeout 10 --max-time 300 @arguments
        if ($LASTEXITCODE -ne 0) { Fail "Download failed ($($arguments[-1]))." }
    }

    # KILN_RELEASE_URL points the installer at a mirror of the GitHub releases layout.
    $repo = if ($env:KILN_RELEASE_URL) { $env:KILN_RELEASE_URL.TrimEnd('/') } else { 'https://github.com/YangTaeyoung/kiln' }
    $latest = Fetch @('-o', 'NUL', '-w', '%{url_effective}', "$repo/releases/latest")
    $version = ($latest -split '/v')[-1]
    if ($version -notmatch '^\d+\.\d+\.\d+$') { Fail 'Cannot resolve the latest stable release.' }
    $archive = "Kiln-$version-windows-$arch.zip"
    $base = "$repo/releases/download/v$version"

    # The archive downloads to the temporary folder; it is extracted next to the installation folder
    # so the final move stays on one volume.
    $work = Join-Path ([IO.Path]::GetTempPath()) ("kiln-install-" + [Guid]::NewGuid().ToString('N'))
    $parent = Split-Path -Parent $installDir
    $staged = Join-Path $parent ('.kiln-install-' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $work | Out-Null
    try {
        Write-Host "Downloading Kiln $version for Windows $arch..."
        Fetch @('-o', (Join-Path $work $archive), "$base/$archive")
        Fetch @('-o', (Join-Path $work 'SHA256SUMS.txt'), "$base/SHA256SUMS.txt")
        $expected = Get-Content -LiteralPath (Join-Path $work 'SHA256SUMS.txt') |
            ForEach-Object { $fields = $_ -split '\s+'; if ($fields.Count -eq 2 -and $fields[1] -eq $archive) { $fields[0] } } |
            Select-Object -First 1
        if ($expected -notmatch '^[0-9a-f]{64}$') { Fail 'Invalid release checksum.' }
        $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $work $archive)).Hash.ToLowerInvariant()
        if ($expected -ne $actual) { Fail 'Release checksum mismatch.' }

        # Refuse paths outside the installation folder before extracting the official archive.
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        $zip = [IO.Compression.ZipFile]::OpenRead((Join-Path $work $archive))
        try {
            foreach ($entry in $zip.Entries) {
                $name = $entry.FullName.Replace('\', '/')
                if ($name.StartsWith('/') -or $name.Contains(':') -or ($name.Split('/') -contains '..')) { Fail 'Unexpected archive contents.' }
            }
        } finally { $zip.Dispose() }
        New-Item -ItemType Directory -Force -Path $parent | Out-Null
        [IO.Compression.ZipFile]::ExtractToDirectory((Join-Path $work $archive), $staged)
        foreach ($required in @('Kiln.exe', 'bin\kiln.exe')) {
            if (-not (Test-Path -LiteralPath (Join-Path $staged $required))) { Fail "The release archive lacks $required." }
        }
        $reported = & (Join-Path $staged 'bin\kiln.exe') --version
        if ("$reported" -notmatch [regex]::Escape($version)) { Fail 'Release version mismatch.' }

        # Never replace an installed Kiln or disrupt its running terminals. Kiln.exe, which marks
        # an installation, arrives last.
        if (Test-Path -LiteralPath (Join-Path $installDir 'Kiln.exe')) { Fail 'Kiln was installed during this download. Use its updater.' }
        if (Test-Path -LiteralPath $installDir) {
            Get-ChildItem -LiteralPath $staged | Where-Object { $_.Name -ne 'Kiln.exe' } |
                ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $installDir -Recurse -Force }
            Move-Item -LiteralPath (Join-Path $staged 'Kiln.exe') -Destination $installDir
        } else {
            Move-Item -LiteralPath $staged -Destination $installDir
        }

        # A Start-menu shortcut carrying Kiln's app ID groups its windows and shows its notifications.
        $programs = [Environment]::GetFolderPath('Programs')
        [KilnInstall]::CreateShortcut((Join-Path $programs 'Kiln.lnk'), (Join-Path $installDir 'Kiln.exe'), $installDir, 'dev.kiln.app')

        # The user Path keeps its value type and unexpanded entries such as %USERPROFILE%\bin.
        $bin = Join-Path $installDir 'bin'
        $environment = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
        try {
            $userPath = [string]$environment.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            $entries = @($userPath -split ';' | Where-Object { $_ })
            if ($entries -notcontains $bin) {
                $kind = if ($null -ne $environment.GetValue('Path')) { $environment.GetValueKind('Path') } else { [Microsoft.Win32.RegistryValueKind]::ExpandString }
                $environment.SetValue('Path', (($entries + $bin) -join ';'), $kind)
                [KilnInstall]::AnnounceEnvironment()
            }
        } finally { $environment.Dispose() }
        Write-Host "Installed Kiln $version. Open Kiln from the Start menu, or run 'kiln' in a new terminal."
    } finally {
        Remove-Item -Recurse -Force -LiteralPath $work -ErrorAction SilentlyContinue
        Remove-Item -Recurse -Force -LiteralPath $staged -ErrorAction SilentlyContinue
    }
}
