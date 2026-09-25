# Kiln

터미널을 중심에 둔 가벼운 IDE. Rust + egui 로 만들었고 macOS · Windows · Linux 에서 돈다.

앱을 끄거나 **업데이트해도 터미널 안에서 돌던 `claude`, `codex`, 빌드, 서버가 죽지 않는다.**
PTY 와 터미널 상태를 GUI 와 분리된 데몬이 들고 있기 때문이다.

## 기능

| 영역 | 내용 |
|---|---|
| 터미널 | 좌우/상하 분할(무제한 중첩, 드래그로 크기 조절, 더블클릭 균등화), 탭, 256색·트루컬러, 한글 IME, 와이드 문자, 마우스 리포팅(SGR), 브래킷 붙여넣기, kitty 키보드 기본 모드, 스크롤백 1만 줄 + 검색, 선택/복사, URL·`파일:줄:열` ⌘클릭 열기 |
| 세션 영속성 | 데몬이 PTY 소유 → 앱 종료·크래시·업데이트 후 재실행 시 레이아웃과 세션 그대로 복원. Unix 는 데몬 자체도 무중단 교체(fd 상속 + exec) |
| 워크스페이스 | 루트 폴더별 워크스페이스, 사이드바에 브랜치·변경 수·PR 상태·실행 중 프로세스·최근 알림 표시(cmux 스타일) |
| 에이전트 알림 | OSC 9 / 777 / 99 알림과 BEL 을 감지해 창 테두리·탭·사이드바 배지, 토스트, OS 알림. ⇧⌘U 로 최근 알림 창으로 이동 |
| 에디터 | 가상화 코드 에디터, syntect + bat 문법 세트(200+ 언어) 증분 하이라이트, 찾기/바꾸기(정규식), 줄 이동, 주석 토글, 자동 들여쓰기, 줄끝·인코딩 보존, 외부 변경 감지 |
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
| 탐색기 · 검색 · Git · PR · DB | ⇧⌘E · ⇧⌘F · ⇧⌘G · ⇧⌘R · ⇧⌘B |
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

- Windows 는 데몬 무중단 교체가 없다. 업데이트 후에도 기존 데몬(이전 버전)이 계속 세션을 들고 있고,
  프로토콜이 호환되는 한 새 GUI 가 그대로 붙는다. 데몬 자체를 새 버전으로 바꾸려면 세션을 닫아야 한다.
- Windows 에서 SSH 세션 안에서 데몬을 처음 띄우면 데몬이 끝날 때까지 SSH 세션이 닫히지 않는다.
- 선택 복사는 화면에 보이는 범위만 대상으로 한다(스크롤하며 선택하면 자동 스크롤).
- 에디터: 자동 줄바꿈·멀티 커서·폴딩·LSP 없음.
- OSC 8 하이퍼링크, 이미지 프로토콜(sixel/kitty graphics) 미지원.
