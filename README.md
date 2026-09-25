# Kiln

터미널을 중심에 둔 가벼운 IDE. Rust + egui 로 만들었고 macOS · Windows · Linux 에서 돈다.

앱을 끄거나 **업데이트해도 터미널 안에서 돌던 `claude`, `codex`, 빌드, 서버가 죽지 않는다.**
PTY 와 터미널 상태를 GUI 와 분리된 데몬이 들고 있기 때문이다.

## 기능

| 영역 | 내용 |
|---|---|
| 터미널 | 좌우/상하 분할(무제한 중첩, 드래그로 크기 조절, 더블클릭 균등화), 탭, 256색·트루컬러, 한글 IME, 와이드 문자, 마우스 리포팅(SGR), 브래킷 붙여넣기, kitty 키보드 기본 모드, 스크롤백 1만 줄 + 검색, 스크롤백까지 걸친 선택 복사, OSC 8 하이퍼링크, 인라인 이미지(iTerm2 `imgcat`·kitty 그래픽·sixel), URL·`파일:줄:열` ⌘클릭 열기 |
| 세션 영속성 | 데몬이 PTY 소유 → 앱 종료·크래시·업데이트 후 재실행 시 레이아웃과 세션 그대로 복원. 데몬 자체도 무중단 교체(Unix: fd 상속 + exec, Windows: pty-host 재부착). 데몬이 비정상 종료돼도(Windows 기본, Unix 는 `KILN_PTY_HOST=1`) 다음 데몬이 세션을 입양 |
| 워크스페이스 | 루트 폴더별 워크스페이스, 사이드바에 브랜치·변경 수·PR 상태·실행 중 프로세스·최근 알림 표시(cmux 스타일) |
| 에이전트 알림 | OSC 9 / 777 / 99 알림과 BEL 을 감지해 창 테두리·탭·사이드바 배지, 토스트, OS 알림. ⇧⌘U 로 최근 알림 창으로 이동 |
| 에디터 | 가상화 코드 에디터, syntect + bat 문법 세트(200+ 언어) 증분 하이라이트, 자동 줄바꿈(⌥Z), 멀티 커서(⌥클릭·⌘D·⌥⌘↑↓), 코드 접기, 찾기/바꾸기(정규식), 줄 이동, 주석 토글, 자동 들여쓰기, 줄끝·인코딩 보존, 외부 변경 감지 |
| LSP | rust-analyzer · gopls · typescript-language-server · pyright/pylsp · clangd · lua-language-server 자동 감지(`lsp.json` 으로 재정의). 진단(물결 밑줄 + 문제 패널 ⇧⌘M), 호버, 정의로 이동(F12·⌘클릭), 참조(⇧F12), 완성, 이름 바꾸기(F2), 포맷(⇧⌥F), 시그니처 도움말 |
| 탐색 | 파일 트리(.gitignore 반영, 실시간 감시, git 상태 색상, 새 파일/이름 변경/휴지통 삭제), ⌘P 퍼지 파일 찾기, 프로젝트 전체 검색/바꾸기 |
| Git | 스테이징/해제/되돌리기, 헝크 단위 스테이징, 커밋·수정 커밋·커밋 후 Push, Fetch/Pull/Push, 브랜치 전환·생성·삭제, 스태시, 커밋 그래프, diff(통합/양쪽) |
| GitHub | `gh` 기반 PR 목록(필터), PR 상세(본문·체크·리뷰·타임라인·파일 diff), 생성, 체크아웃, 승인/변경 요청/코멘트, 머지(squash/merge/rebase), Ready for review |
| 데이터베이스 | Postgres · MySQL/MariaDB · SQLite. 스키마 트리, 테이블 그리드(페이지, 정렬, WHERE/ORDER BY, 셀 편집·행 추가/삭제를 PK 기준 한 트랜잭션으로 제출), SQL 콘솔(구문 단위 실행, 취소, EXPLAIN, 기록), DDL/구조 보기, CSV/JSON 내보내기, 비밀번호는 OS 키체인 |
| 기타 | 명령 팔레트(⇧⌘P), 설정, 상태 바, CLI 자동화 |

## 설치

### macOS
```bash
scripts/bundle-macos.sh --install   # /Applications/Kiln.app + ~/.local/bin/kiln
```
업데이트도 같은 명령이다. 실행 중인 세션은 유지되고, 새 앱이 켜지면 데몬이 새 바이너리로 교체된다.

### Linux
```bash
sudo apt install libxkbcommon-dev libwayland-dev   # 빌드에 필요
cargo build --release -p kiln
install -m755 target/release/kiln ~/.local/bin/kiln
```

### Windows
```powershell
cargo build --release -p kiln   # CRT 정적 링크(.cargo/config.toml)
```
macOS 에서 교차 빌드: `cargo xwin build --cross-compiler clang --release --target aarch64-pc-windows-msvc -p kiln`
(llvm, cargo-xwin 필요). 데몬은 `%LOCALAPPDATA%\Kiln\daemon\` 의 빌드별 복사본에서 돌아서 `kiln.exe` 를 언제든 교체할 수 있다.
세션마다 `kiln pty-host` 프로세스가 ConPTY 와 자식을 들고 있어, 업데이트 후 새 앱이 켜지면 데몬이 새 버전으로 교체되고 세션은 그대로 이어진다.

## 단축키

macOS 는 ⌘, Linux/Windows 는 Ctrl+Shift(⇧ 조합은 Ctrl+Shift+Alt).

| 동작 | 키 |
|---|---|
| 새 터미널 탭 / 창 닫기 | ⌘T / ⌘W |
| 오른쪽·아래로 분할 | ⌘D / ⇧⌘D |
| 분할 창 이동 / 균등화 | ⌥⌘←↑→↓ / ⌥⌘= |
| 워크스페이스 전환 / 새 워크스페이스 | ⌘1…9 / ⌘N |
| 탭 전환 | ⌥⌘1…9, ⇧⌘[ ⇧⌘] |
| 파일 빠르게 열기 / 명령 팔레트 | ⌘P / ⇧⌘P (⌘K) |
| 탐색기 · 검색 · Git · PR · DB · 문제 | ⇧⌘E · ⇧⌘F · ⇧⌘G · ⇧⌘R · ⇧⌘B · ⇧⌘M |
| 터미널/에디터에서 찾기 | ⌘F |
| 최근 알림으로 이동 | ⇧⌘U |
| 사이드바 / 글꼴 크기 | ⌘B / ⌘= ⌘- ⌘0 |

터미널: Shift/Option+Enter 는 `ESC CR`(에이전트 CLI 줄바꿈), Option 은 기본 Meta.

## 에이전트 알림 연결

Kiln 은 OSC 9/777/99 와 BEL 을 모두 알림으로 받는다. 에이전트가 이 중 하나를 보내지 않는 설정이라면
훅에서 `kiln notify` 를 부르면 된다(세션 안에서 실행하면 그 세션의 알림이 된다). Claude Code 예:

```jsonc
// ~/.claude/settings.json
{ "hooks": { "Notification": [ { "hooks": [ { "type": "command", "command": "kiln notify Claude \"입력이 필요합니다\"" } ] } ] } }
```

## CLI

```
kiln [경로]                 GUI 실행(경로를 워크스페이스로)
kiln ls [--json]            세션 목록
kiln new [--cwd D] [-- cmd] 세션 생성
kiln send <id> <텍스트> -e  입력(Enter 포함)
kiln read <id> [--history N] 화면/스크롤백 읽기
kiln kill <id>
kiln notify <제목> [본문]   현재 터미널에 알림
kiln status | upgrade-daemon | shutdown-daemon
```

## 구조

```
crates/
  kiln-proto   데몬 ↔ 클라이언트 메시지(postcard, 길이 접두 프레임)
  kiln-daemon  PTY(Unix: openpty / Windows: ConPTY), alacritty_terminal 에뮬레이션,
               변경된 줄만 보내는 프레임 푸시(최소 4ms 간격), OSC 알림 파서, 무중단 업그레이드
  kiln         egui GUI(워크스페이스·탭·분할·터미널 렌더러) + CLI + `kiln daemon`
  kiln-editor  에디터, 파일 트리, 빠른 열기, 프로젝트 검색
  kiln-git     git/gh CLI 백엔드, 소스 제어·diff·PR 화면
  kiln-db      sqlx 백엔드, DB 탐색기·그리드·콘솔
  kiln-common  테마, 백그라운드 작업, 설정 저장
```

**Windows(pty-host).** 세션마다 `kiln pty-host` 가 ConPTY 와 자식 프로세스를 소유하고, 데몬은 호스트에 로컬 파이프로
붙어 중계한다. 업그레이드 때 데몬은 호스트에서 떨어지고 화면 상태를 파일로 남긴 뒤 새 데몬을 띄우고 종료한다. 새 데몬은
이전 데몬이 끝나기를 기다렸다가 같은 이름으로 리슨하고 호스트에 다시 붙는다. 그 사이 출력은 호스트가 버퍼에 모아 두었다가
넘긴다. 데몬이 강제 종료돼도 호스트 목록(레지스트리)이 남아 있어 다음 데몬이 호스트를 입양하고 최근 출력을 재생해 화면을
되살린다. 파이프 DACL 은 현재 사용자 SID 에만 권한을 준다.

**영속성 흐름.** GUI 는 로컬 소켓(Unix 소켓 / 네임드 파이프)으로 데몬에 붙어 화면 프레임만 받는다.
GUI 가 죽어도 데몬과 PTY 는 그대로다. 새 GUI 는 저장된 레이아웃(`state.json`)의 세션 id 로 다시 붙는다.
실행 파일이 바뀌어 빌드 id 가 다르면 GUI 가 데몬에 `Upgrade` 를 보내고, 데몬은 읽기 스레드를 멈춘 뒤
각 세션의 그리드(스크롤백·대체 화면·모드)를 직렬화하고 PTY fd 와 리스닝 소켓을 상속한 채 새 바이너리로
`exec` 한다. pid 가 그대로라 자식 프로세스는 아무 영향도 받지 않는다.

## 테스트

```bash
cargo test --workspace                          # 단위·통합·GUI(egui_kittest) 테스트
crates/kiln-db/tests/run_docker_tests.sh        # Postgres 16 / MySQL 8.4 컨테이너 테스트
cargo test --release -p kiln --test render_perf -- --nocapture
```

`crates/kiln/tests/daemon.rs` 는 실제 바이너리로 데몬을 띄워 클라이언트 재접속, 무중단 업그레이드
(셸 pid·백그라운드 작업·화면 유지), 알림, 증분 프레임, 종료 코드를 확인한다.

## 알려진 한계

- 데몬 비정상 종료 후 입양된 세션은 호스트가 보관한 최근 출력(1MB)을 다시 재생해 화면을 만든다. 전체 화면 TUI
  (vim 등)는 다음 다시 그리기 전까지 잠깐 어긋나 보일 수 있다. 정상 업그레이드는 화면 상태를 그대로 옮기므로 해당 없음.
- 에디터 멀티 커서: 실행 취소는 기본 커서만 복원하고, 줄 단위 명령(주석 토글·줄 이동)과 완성 적용은 보조 커서를 해제한다.
- kitty 그래픽은 직접 전송(`t=d`)만 지원한다(파일·공유 메모리 전송 미지원).
