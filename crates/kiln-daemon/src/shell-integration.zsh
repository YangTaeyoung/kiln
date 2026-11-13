# Standard OSC 133 boundaries; command text is hex to keep control bytes out of OSC.
# Plain local Codex sessions use an embedded server. Explicit commands and user wrappers stay intact.
if (( ! $+functions[codex] && ! $+aliases[codex] )); then
  function codex {
    case "${1:-}" in
      ''|resume|fork)
        local arg
        for arg in "$@"; do
          case "$arg" in
            --remote|--remote=*|--no-daemon) command codex "$@"; return $? ;;
            --) break ;;
          esac
        done
        local launcher=${KILN_AGENT_LAUNCHER:-}
        if [[ -n $launcher && -x $launcher ]]; then
          "$launcher" agent-shell --tool codex --cwd "$PWD" -- --no-daemon "$@"
        else
          command codex --no-daemon "$@"
        fi
        ;;
      *) command codex "$@" ;;
    esac
  }
fi
# Preserve user aliases and functions. Authentication commands use the native CLI.
if (( ! $+functions[claude] && ! $+aliases[claude] )); then
  function claude {
    local launcher=${KILN_AGENT_LAUNCHER:-}
    case "${1:-}" in
      --version|-v|--help|-h|auth|login|logout|update|upgrade|install|doctor|mcp)
        command claude "$@"; return $? ;;
    esac
    if [[ -n $launcher && -x $launcher ]]; then
      "$launcher" agent-shell --tool claude --cwd "$PWD" -- "$@"
    else
      command claude "$@"
    fi
  }
fi
# Only Kiln's shells receive this wrapper; the ordinary CLI login stays intact.
function agy {
  local launcher=${KILN_AGENT_LAUNCHER:-}
  case "${1:-}" in
    --version|-v|--help|-h|update) command agy "$@"; return $? ;;
  esac
  if [[ -n $launcher && -x $launcher ]]; then
    "$launcher" agent-shell --cwd "$PWD" -- "$@"
  else
    command agy "$@"
  fi
}
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

# Completion previews are limited to safe built-in contexts; explicit requests are separate.
# Only the ZLE widget reads or replaces BUFFER; neither side evaluates shell text.
_kiln_complete_hex() {
  emulate -L zsh
  local LC_ALL=C value=$1 result='' part i
  for (( i=1; i<=${#value}; i++ )); do
    printf -v part '%02x' "'${value[i]}"
    result+=$part
  done
  REPLY=$result
}
# A simple final $NAME or ${NAME prefix; never send parameter values.
_kiln_variable_prefix() {
  local tail=${1##*[[:space:]]}
  local lead=${1%"$tail"}
  [[ $lead != *[\$\`\;\|\&\(\)\<\>\"\'\\]* ]] || return 1
  case "$tail" in
    '$'*) REPLY=${tail#\$}; REPLY=${REPLY#\{} ;;
    *) return 1 ;;
  esac
  [[ -z $REPLY || $REPLY == [a-zA-Z_][a-zA-Z0-9_]# ]]
}
_kiln_complete_preview() {
  emulate -L zsh
  setopt localoptions extendedglob
  [[ ${KILN_COMPLETION_PREVIEW:-1} != 0 ]] || return
  local snapshot="$CURSOR:$PWD:$BUFFER"
  [[ $snapshot == ${_KILN_LAST_PREVIEW:-} ]] && return
  _KILN_LAST_PREVIEW=$snapshot
  # Forward plain edit contexts, never evaluate them. Native shell completion
  # retains expansions, pipelines, redirections and editing before the caret.
  local command=${BUFFER%%[[:space:]]*}
  if [[ $CURSOR == ${#BUFFER} && ${#BUFFER} -le 1024 ]] && _kiln_variable_prefix "$BUFFER"; then
    _kiln_complete_request preview
  elif [[ -n $command && $command == [a-zA-Z0-9_+./-]## && $CURSOR == ${#BUFFER} && ${#BUFFER} -le 1024 &&
        $BUFFER != *[\;\|\&\$\`\(\)\<\>]* && $BUFFER != *$'\n'* && $BUFFER != *$'\r'* &&
        ( $BUFFER == *[[:space:]]* || ${#command} -ge 2 ) ]]; then
    _kiln_complete_request preview
  else
    printf '\e]777;kiln-complete-cancel\a'
  fi
}
_kiln_complete_request() {
  emulate -L zsh
  setopt localoptions extendedglob
  (( ${#BUFFER} <= 1024 )) || return
  if [[ -z ${_KILN_COMPLETION_DIR:-} ]]; then
    _KILN_COMPLETION_DIR=$(/usr/bin/mktemp -d "${TMPDIR:-/tmp}/kiln-completion.XXXXXXXX") || return
    /bin/chmod 700 "$_KILN_COMPLETION_DIR"
  fi
  (( _KILN_COMPLETION_REV++ ))
  _KILN_COMPLETION_BUFFER=$BUFFER
  _KILN_COMPLETION_CURSOR=$CURSOR
  local file="$_KILN_COMPLETION_DIR/request" buffer cwd names='' pathhex explicit=1
  [[ $1 == preview ]] && explicit=0
  _kiln_complete_hex "$BUFFER"; buffer=$REPLY
  _kiln_complete_hex "$PWD"; cwd=$REPLY
  local prefix=${BUFFER[1,CURSOR]}
  local -a matches
  local -a metadata
  local name pattern
  if _kiln_variable_prefix "$prefix"; then
    pattern="${REPLY}*"
    matches=(${(ok)parameters})
    for name in "${matches[@]}"; do
      [[ $name == ${~pattern} && $name == [a-zA-Z_][a-zA-Z0-9_]# && ${parameters[$name]} == *export* ]] || continue
      metadata+=("variable:$name")
      (( ${#metadata} >= 80 )) && break
    done
    _kiln_complete_hex "${(F)metadata}"; names=$REPLY
  elif [[ $prefix == [a-zA-Z0-9_+.-]# ]]; then
    pattern="${prefix}*"
    for name in ${(ok)aliases}; do
      [[ $name == ${~pattern} && $name == [a-zA-Z0-9_+.-]## ]] && metadata+=("alias:$name")
      (( ${#metadata} >= 40 )) && break
    done
    for name in ${(ok)functions}; do
      [[ $name != _* && $name == ${~pattern} && $name == [a-zA-Z0-9_+.-]## ]] && metadata+=("function:$name")
      (( ${#metadata} >= 60 )) && break
    done
    matches=(${(f)$(builtin whence -pm -- "${prefix}*" 2>/dev/null)})
    metadata+=(${matches:t})
    _kiln_complete_hex "${(F)metadata[1,80]}"; names=$REPLY
  else
    # The shell supplies its actual PATH resolution. The GUI never guesses an
    # executable from its own PATH and receives no command argument values here.
    local command=${BUFFER%%[[:space:]]*} executable
    if [[ $command == [a-zA-Z0-9_+.-]## ]]; then
      executable=$(builtin whence -p -- "$command" 2>/dev/null)
      [[ $executable == /* && $executable != *$'\n'* ]] && { _kiln_complete_hex "$executable"; names=$REPLY; }
    fi
  fi
  _kiln_complete_hex "$file"; pathhex=$REPLY
  while (( ${#buffer}+${#cwd}+${#pathhex}+${#names} >= 7900 && ${#metadata} > 1 )); do
    metadata[-1]=()
    _kiln_complete_hex "${(F)metadata[1,80]}"; names=$REPLY
  done
  (( ${#buffer}+${#cwd}+${#pathhex}+${#names} < 7900 )) || return
  printf '\e]777;kiln-complete;%s;%s;%s;%s;%s;%s;%s\a' "$_KILN_COMPLETION_REV" "$CURSOR" "$buffer" "$cwd" "$pathhex" "$names" "$explicit"
}
_kiln_complete_accept() {
  emulate -L zsh
  local file="${_KILN_COMPLETION_DIR:-}/request"
  [[ -n ${_KILN_COMPLETION_DIR:-} && -f $file && ! -L $file ]] || return
  local revision cursor text
  { IFS= read -r revision; IFS= read -r cursor; IFS= read -r text; } < "$file"
  /bin/rm -f -- "$file"
  [[ $revision == ${_KILN_COMPLETION_REV:-} && $BUFFER == "$_KILN_COMPLETION_BUFFER" && $CURSOR == $_KILN_COMPLETION_CURSOR ]] || return
  [[ $cursor == <-> && $text != *[^0-9a-f]* && ${#text} -le 8192 ]] || return
  local decoded=$(printf '%s' "$text" | /usr/bin/xxd -r -p)
  [[ $decoded != *$'\n'* && $decoded != *$'\r'* && $cursor -le ${#decoded} ]] || return
  BUFFER=$decoded
  CURSOR=$cursor
  (( _KILN_COMPLETION_REV++ ))
  zle -R
}
_kiln_complete_finish() {
  emulate -L zsh
  (( _KILN_COMPLETION_REV++ ))
  unset _KILN_COMPLETION_BUFFER _KILN_COMPLETION_CURSOR _KILN_LAST_PREVIEW
  printf '\e]777;kiln-complete-cancel\a'
  [[ -n ${_KILN_COMPLETION_DIR:-} ]] && /bin/rm -f -- "$_KILN_COMPLETION_DIR/request"
}
_kiln_complete_cleanup() {
  emulate -L zsh
  [[ -n ${_KILN_COMPLETION_DIR:-} ]] && /bin/rm -f -- "$_KILN_COMPLETION_DIR/request"
  [[ -n ${_KILN_COMPLETION_DIR:-} ]] && /bin/rmdir -- "$_KILN_COMPLETION_DIR" 2>/dev/null
}
autoload -Uz add-zle-hook-widget
zle -N _kiln_complete_request
zle -N _kiln_complete_accept
bindkey -M emacs '^@' _kiln_complete_request
bindkey -M viins '^@' _kiln_complete_request
bindkey -M emacs '\e[99;1~' _kiln_complete_accept
bindkey -M viins '\e[99;1~' _kiln_complete_accept
add-zle-hook-widget line-pre-redraw _kiln_complete_preview
add-zle-hook-widget line-finish _kiln_complete_finish
add-zsh-hook zshexit _kiln_complete_cleanup
