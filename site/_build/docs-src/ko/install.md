---
title: 설치와 업데이트
description: macOS나 Windows에 Kiln을 설치하고, 첫 워크스페이스를 열고, 앱을 최신 상태로 유지하고, 언어를 고릅니다.
---

Kiln은 명령 한 줄로 설치하거나 GitHub Releases에서 내려받을 수 있습니다. 설치한 뒤에는 Kiln이 알아서 업데이트하며, 업데이트해도 실행 중인 터미널은 그대로입니다.

## 요구 사항 {#requirements}

| 플랫폼 | 요구 사항 |
| --- | --- |
| macOS | Apple Silicon Mac, macOS 11 이상. 빌드는 공증(notarization)을 거칩니다. |
| Windows | x64 또는 ARM64 기반 Windows 11. 빌드는 아직 코드 서명이 되어 있지 않습니다. |

Intel Mac과 Linux용 빌드는 현재 제공하지 않습니다. Windows 10에서는 테스트하지 않았습니다.

## macOS에 설치하기 {#install-on-macos}

### 설치 스크립트로 설치 {#use-the-install-script}

터미널에서 다음 명령을 실행합니다.

```sh
curl -fsSL https://raw.githubusercontent.com/YangTaeyoung/kiln/main/install.sh | sh
```

스크립트는 최신 안정 버전의 Apple Silicon 앱을 내려받은 뒤 릴리스 체크섬, Developer ID 서명, 번들 식별 정보, Apple 공증을 확인하고 `/Applications`에 설치합니다. Kiln이 이미 설치되어 있으면 기존 설치는 건드리지 않습니다. 이때 업그레이드하려면 **Kiln → 업데이트 확인**을 사용하세요.

다른 위치에 설치하려면 `KILN_INSTALL_DIR`을 지정합니다.

```sh
curl -fsSL https://raw.githubusercontent.com/YangTaeyoung/kiln/main/install.sh | KILN_INSTALL_DIR="$HOME/Applications" sh
```

### 앱 직접 내려받기 {#download-the-app}

1. [GitHub Releases](https://github.com/YangTaeyoung/kiln/releases/latest)에서 Apple Silicon용 압축 파일을 내려받습니다.
2. 압축을 풉니다.
3. 앱을 열기 전에 **Kiln.app**을 **응용 프로그램(Applications)** 폴더로 옮깁니다.

## Windows에 설치하기 {#install-on-windows}

### PowerShell 스크립트로 설치 {#use-the-powershell-script}

PowerShell에서 다음 명령을 실행합니다.

```powershell
irm https://raw.githubusercontent.com/YangTaeyoung/kiln/main/install.ps1 | iex
```

스크립트는 컴퓨터에 맞는 x64 또는 ARM64 압축 파일을 고르고, 릴리스 체크섬과 압축 파일의 경로·버전을 확인한 다음 아래 작업을 합니다.

- `%LOCALAPPDATA%\Programs\Kiln`에 Kiln을 설치합니다.
- 시작 메뉴에 **Kiln** 바로 가기를 만듭니다.
- `kiln` 명령을 쓸 수 있도록 `bin` 폴더를 사용자 `PATH`에 추가합니다.

다른 폴더에 설치하려면 `KILN_INSTALL_DIR`을 지정하세요. 이미 설치되어 있으면 기존 설치를 그대로 둡니다.

### 압축 파일 직접 내려받기 {#download-the-archive}

[GitHub Releases](https://github.com/YangTaeyoung/kiln/releases/latest)에서 아키텍처에 맞는 압축 파일을 직접 내려받아도 됩니다.

> **참고:** Windows 빌드는 아직 코드 서명이 되어 있지 않습니다. 설치 스크립트로 내려받으면 SmartScreen 경고가 뜨지 않지만, 브라우저로 내려받은 압축 파일에서는 뜰 수 있습니다. **추가 정보 → 실행(More info → Run anyway)**은 Releases 페이지에서 받은 압축 파일일 때만 선택하세요.

## 처음 실행하기 {#first-launch}

1. 응용 프로그램 폴더나 시작 메뉴에서 **Kiln**을 엽니다.
2. 워크스페이스 사이드바의 **+** 버튼(macOS에서는 `⌘N`)으로 워크스페이스를 만듭니다. 이름을 정하고 프로젝트 폴더를 선택합니다.
3. 터미널을 바로 쓰거나, **새 작업**을 선택해 요청을 입력하고 에이전트를 골라 시작합니다.

폴더는 저장소 하나여도 되고, 여러 저장소를 담은 상위 폴더여도 됩니다. 워크스페이스, 작업, 분할 화면이 어떻게 맞물리는지는 [워크스페이스와 작업](workspaces.html)에서 설명합니다.

Windows에서는 Kiln이 실행 중일 때 다시 실행하면 기존 창이 앞으로 나오고, 넘겨준 폴더를 그 창에서 엽니다.

## 사용할 도구 설치하기 {#install-the-tools-you-use}

Kiln은 이미 설치된 에이전트 CLI와 GitHub CLI를 실행합니다. 이 도구들은 Kiln에 포함되어 있지 않고, 구독도 포함되어 있지 않습니다. 사용할 도구를 설치하고 로그인하세요.

| 도구 | 용도 |
| --- | --- |
| Claude Code (`claude`) | Claude 작업 |
| Codex (`codex`) | Codex 작업 |
| Antigravity (`agy`) | Antigravity 작업. Google 설치 프로그램으로 설치합니다. |
| `gh` | GitHub 기능 |

Kiln은 `PATH`, `~/.local/bin`, `/opt/homebrew/bin`, `/usr/local/bin`에서 `agy`를 찾습니다. Windows에서 Claude Code와 Codex는 네이티브 Windows 프로그램으로 실행되며, 데이터는 `%USERPROFILE%\.codex`와 `%USERPROFILE%\.claude`에 저장합니다.

Claude Code와 Codex 계정은 **설정 → 계정**에서, macOS의 Antigravity는 **설정 → 계정 → Antigravity**에서 로그인할 수 있습니다. Windows에서는 터미널에서 `agy`를 실행해 Antigravity에 로그인합니다. 자세한 내용은 [계정](accounts.html)을 참고하세요. 설치된 CLI 버전을 확인하거나 업데이트하려면 **설정 → 계정 → CLI 업데이트**를 여세요. [대화와 업데이트](conversations.html)에서 자세히 설명합니다.

## Kiln 업데이트 {#update-kiln}

업데이트해도 실행 중인 터미널 세션은 그대로 유지됩니다.

### macOS {#macos}

Kiln은 업데이트를 자동으로 확인하고 Sparkle 업데이트 대화상자로 설치합니다. 직접 확인하려면 **Kiln → 업데이트 확인**을 선택하거나 **설정 → 앱 정보**의 업데이트 항목을 사용하세요. 자동 확인은 설정에서 끌 수 있습니다.

### Windows {#windows}

Kiln은 하루에 한 번 업데이트를 확인합니다. 직접 확인하려면 **설정 → 앱 정보 → 업데이트 확인**을 연 다음 **설치하고 다시 시작**을 선택합니다. Kiln은 파일을 교체하기 전에 내려받은 압축 파일이 릴리스 서명 키와 맞는지 검증합니다. 실행 중인 터미널은 계속 실행됩니다.

> **팁:** macOS 15 이상에서, 업데이트 전부터 열려 있던 패널에서 로컬 네트워크에 접근하는 명령이 실패하면 새 터미널 패널을 열고 다시 실행하세요. [macOS의 로컬 네트워크 접근](terminal.html#local-network-access-on-macos)을 참고하세요.

## 언어 선택하기 {#choose-a-language}

**설정 → 모양 → 언어**를 열고 영어, 한국어, 일본어, 중국어 간체 중 하나를 고릅니다. 변경 사항은 바로 적용되고, 다음에 실행할 때도 유지됩니다.

바뀌는 것은 Kiln 자체의 인터페이스뿐입니다. 터미널 출력, 파일 이름, 코드, 커밋 메시지, 에이전트 요청은 원래 내용 그대로입니다. 이전 버전에서 설치했다면 언어를 고르기 전까지 한국어로 표시됩니다.

## 다음 단계 {#next-steps}

- [워크스페이스와 작업](workspaces.html): 폴더와 작업 정리하기
- [에이전트](agents.html): 첫 에이전트 작업 시작하기
- [터미널](terminal.html): 셸 통합과 활동 보고 설정하기
