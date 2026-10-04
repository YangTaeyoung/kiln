# Standard OSC 133 boundaries; command text is hex to keep control bytes out of OSC.
autoload -Uz add-zsh-hook
_kiln_preexec() {
  local hex=$(printf '%s' "$1" | /usr/bin/od -An -tx1 | /usr/bin/tr -d ' \n')
  printf '\e]777;kiln-command;%s\a\e]133;C\a' "$hex"
  typeset -g _KILN_COMMAND_ACTIVE=1
}
_kiln_precmd() {
  local result=$?
  if [[ ${_KILN_COMMAND_ACTIVE:-0} == 1 ]]; then
    printf '\e]133;D;%s\a' "$result"
    typeset -g _KILN_COMMAND_ACTIVE=0
  fi
  printf '\e]133;A\a'
}
add-zsh-hook preexec _kiln_preexec
preexec_functions=(_kiln_preexec ${preexec_functions:#_kiln_preexec})
add-zsh-hook precmd _kiln_precmd
precmd_functions=(_kiln_precmd ${precmd_functions:#_kiln_precmd})
# Hooks can call kiln-agent-status with an explicit lifecycle event; no output heuristics.
kiln-agent-status() {
  case "$1" in
    running|waiting|done|failed|unknown) printf '\e]777;kiln-agent;%s\a' "$1" ;;
    *) printf 'Usage: kiln-agent-status running|waiting|done|failed|unknown\n' >&2; return 2 ;;
  esac
}
