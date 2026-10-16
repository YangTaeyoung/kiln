# Kiln shell integration for Windows PowerShell 5.1 and PowerShell 7.
# Emits the sequences of shell-integration.zsh: the command record and start before a command,
# its exit code and the prompt mark at the next prompt, and OSC 7 with the current location.
# The user's prompt function and profile stay in effect; this script defines only globals
# prefixed with `__Kiln` and the `kiln-agent-status` command.
if ($global:__KilnIntegration) { return }
$global:__KilnIntegration = $true
$global:__KilnCommandRan = $false
$global:__KilnPrompt = $function:prompt

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
    $text = & $global:__KilnPrompt
    $global:LASTEXITCODE = $code
    return $marks + $text
}

if (Get-Module -Name PSReadLine) {
    function global:PSConsoleHostReadLine {
        $line = [Microsoft.PowerShell.PSConsoleReadLine]::ReadLine($Host.Runspace, $ExecutionContext)
        if ($line.Trim().Length -gt 0) {
            [Console]::Write("$([char]27)]777;kiln-command;$(__KilnHex $line)$([char]7)$([char]27)]133;C$([char]7)")
            $global:__KilnCommandRan = $true
        }
        $line
    }
}
