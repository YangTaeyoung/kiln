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
_kiln_complete_preview() {
  emulate -L zsh
  setopt localoptions extendedglob
  [[ ${KILN_COMPLETION_PREVIEW:-1} != 0 ]] || return
  [[ $BUFFER == ${_KILN_LAST_PREVIEW:-} ]] && return
  _KILN_LAST_PREVIEW=$BUFFER
  if [[ $BUFFER == 'cd '* || $BUFFER == (git|npm|docker|cargo|ls)' '[a-zA-Z0-9-]# ]]; then
    [[ $BUFFER != *[\;\|\&\$\`\(\)\<\>]* && $BUFFER != *$'\n'* ]] || { printf '\e]777;kiln-complete-cancel\a'; return; }
    if [[ $BUFFER != 'cd '* ]]; then
      local command=${BUFFER%% *} prefix=${BUFFER#* } option matched=0
      local -a options
      case $command in
        git) options=(status diff log switch add commit --help) ;;
        docker) options=(ps images build run compose --help) ;;
        npm) options=(install run test --help) ;;
        cargo) options=(check build test run --help) ;;
        ls) options=(-a -l -h) ;;
      esac
      for option in $options; do [[ $option == "$prefix"* ]] && matched=1; done
      if (( ! matched )); then printf '\e]777;kiln-complete-cancel\a'; return; fi
    fi
    _kiln_complete_request preview
  else
    printf '\e]777;kiln-complete-cancel\a'
  fi
}
_kiln_complete_request() {
  emulate -L zsh
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
  if [[ $explicit == 1 && $prefix != *[[:space:]]* && $prefix != *[\;\|\&\$\`\(\)]* ]]; then
    matches=(${(f)$(builtin whence -pm -- "${prefix}*" 2>/dev/null)})
    matches=(${matches:t})
    local names_text="${(F)matches[1,80]}"
    _kiln_complete_hex "$names_text"; names=${REPLY[1,2048]}
  fi
  _kiln_complete_hex "$file"; pathhex=$REPLY
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
