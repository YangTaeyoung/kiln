# Kiln shell integration for Windows PowerShell 5.1 and PowerShell 7.
# Emits the sequences of shell-integration.zsh: the command record and start before a command,
# its exit code and the prompt mark at the next prompt, and OSC 7 with the current location.
# The user's prompt and line-reading functions and profile stay in effect; this script defines
# only globals prefixed with `__Kiln` and the `kiln-agent-status` command.
if ($global:__KilnIntegration) { return }
$global:__KilnIntegration = $true
$global:__KilnCommandRan = $false
$global:__KilnPrompt = $function:prompt

# Keep user-defined aliases/functions and native CLI management commands.
if (-not (Get-Command claude -CommandType Alias,Function -ErrorAction SilentlyContinue)) {
    function global:claude {
        $launcher = $env:KILN_AGENT_LAUNCHER
        if ($launcher -and (Test-Path -LiteralPath $launcher) -and $PWD.Provider.Name -eq 'FileSystem' -and
            $args[0] -notin @('--version','-v','--help','-h','auth','login','logout','update','upgrade','install','doctor','mcp')) {
            & $launcher agent-shell --tool claude --cwd $PWD.Path -- @args
        } else {
            $native = Get-Command claude -CommandType Application,ExternalScript -ErrorAction Stop | Select-Object -First 1
            & $native.Source @args
        }
    }
}

if (-not (Get-Command codex -CommandType Alias,Function -ErrorAction SilentlyContinue)) {
    function global:codex {
        $launcher = $env:KILN_AGENT_LAUNCHER
        if ($launcher -and (Test-Path -LiteralPath $launcher) -and $PWD.Provider.Name -eq 'FileSystem' -and
            ($args.Count -eq 0 -or $args[0] -in @('resume','fork')) -and
            -not ($args -like '--remote*')) {
            & $launcher agent-shell --tool codex --cwd $PWD.Path -- @args
        } else {
            $native = Get-Command codex -CommandType Application,ExternalScript -ErrorAction Stop | Select-Object -First 1
            & $native.Source @args
        }
    }
}

function global:kiln-agent-status {
    param([Parameter(Mandatory)][ValidateSet('running', 'waiting', 'done', 'failed', 'unknown')][string]$State)
    [Console]::Write("$([char]27)]777;kiln-agent;$State$([char]7)")
}

function global:__KilnHex([string]$Text) {
    -join ([Text.Encoding]::UTF8.GetBytes($Text) | ForEach-Object { $_.ToString('x2') })
}

function global:prompt {
    $succeeded = $global:?
    $code = $global:LASTEXITCODE
    $esc = [char]27
    $bel = [char]7
    $marks = ''
    if ($global:__KilnCommandRan) {
        $exit = 0
        if (-not $succeeded) { if ($code) { $exit = $code } else { $exit = 1 } }
        $marks += "$esc]133;D;$exit$bel"
        $global:__KilnCommandRan = $false
    }
    $location = $ExecutionContext.SessionState.Path.CurrentLocation
    if ($location.Provider.Name -eq 'FileSystem') {
        $segments = $location.ProviderPath.Replace('\', '/').Split('/') | ForEach-Object { [Uri]::EscapeDataString($_) }
        $marks += "$esc]7;file://localhost/$($segments -join '/')$bel"
    }
    $marks += "$esc]133;A$bel"
    # The original prompt reads the last command's status from $?, which the lines above reset.
    if (-not $succeeded) { Write-Error -Message 'failed' -ErrorAction Ignore }
    $text = & $global:__KilnPrompt
    $global:LASTEXITCODE = $code
    return $marks + $text
}

# PSReadLine, or a tool that wraps it, defines PSConsoleHostReadLine; it keeps reading the line
# and reads $? first, so it is called before anything else runs.
if (Test-Path Function:\PSConsoleHostReadLine) {
    $global:__KilnReadLine = $function:PSConsoleHostReadLine
    function global:PSConsoleHostReadLine {
        $line = & $global:__KilnReadLine
        if ($line -and $line.Trim().Length -gt 0) {
            [Console]::Write("$([char]27)]777;kiln-command;$(__KilnHex $line)$([char]7)$([char]27)]133;C$([char]7)")
            $global:__KilnCommandRan = $true
        }
        $line
    }
}
