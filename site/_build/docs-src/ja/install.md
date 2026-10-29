---
title: インストールとアップデート
description: macOS と Windows への Kiln のインストール、最初のワークスペースの作成、アップデート、表示言語の設定について説明します。
---

Kiln はコマンド 1 つでインストールできます。GitHub Releases からダウンロードしてもかまいません。インストール後は Kiln が自動でアップデートされ、実行中のターミナルはアップデートの影響を受けません。

ウェブサイトのダウンロードボタンは、ブラウザーが示す OS を優先して表示します。もう一方の OS も選べます。モバイル端末や OS を判別できない場合は両方を表示します。ボタンから GitHub Releases を開き、お使いのコンピューターに合うアーカイブを選んでください。

## 動作環境 {#requirements}

| プラットフォーム | 要件 |
| --- | --- |
| macOS | Apple Silicon 搭載 Mac の macOS 11 以降。ビルドは Apple の公証（notarization）済みです。 |
| Windows | x64 または ARM64 の Windows 11。ビルドはまだコード署名されていません。 |

Intel Mac 版と Linux 版は現在提供していません。Windows 10 での動作は検証していません。

## macOS へのインストール {#install-on-macos}

### インストールスクリプトを使う {#use-the-install-script}

ターミナルで次のコマンドを実行します。

```sh
curl -fsSL https://raw.githubusercontent.com/YangTaeyoung/kiln/main/install.sh | sh
```

スクリプトは最新の安定版 Apple Silicon アプリをダウンロードし、リリースのチェックサム、Developer ID 署名、バンドルの識別情報、Apple の公証を確認してから `/Applications` にインストールします。Kiln がすでにインストールされている場合、スクリプトは既存のアプリに手を加えません。アップグレードには **Kiln → アップデートを確認…** を使ってください。

別の場所にインストールするには `KILN_INSTALL_DIR` を指定します。

```sh
curl -fsSL https://raw.githubusercontent.com/YangTaeyoung/kiln/main/install.sh | KILN_INSTALL_DIR="$HOME/Applications" sh
```

### アプリをダウンロードする {#download-the-app}

1. [GitHub Releases](https://github.com/YangTaeyoung/kiln/releases/latest) から Apple Silicon 用のアーカイブをダウンロードします。
2. アーカイブを展開します。
3. **Kiln.app** を**アプリケーション**フォルダーに移動してから開きます。

## Windows へのインストール {#install-on-windows}

### PowerShell スクリプトを使う {#use-the-powershell-script}

PowerShell で次のコマンドを実行します。

```powershell
irm https://raw.githubusercontent.com/YangTaeyoung/kiln/main/install.ps1 | iex
```

スクリプトはマシンに合った x64 または ARM64 のアーカイブを選び、リリースのチェックサムと、アーカイブ内のパスおよびバージョンを確認したうえで、次の処理を行います。

- Kiln を `%LOCALAPPDATA%\Programs\Kiln` にインストールします。
- スタートメニューに **Kiln** のショートカットを追加します。
- `kiln` コマンドを使えるように、`bin` フォルダーをユーザーの `PATH` に追加します。

別のフォルダーにインストールするには `KILN_INSTALL_DIR` を指定します。既存のインストールがある場合は、そのまま残します。

### アーカイブをダウンロードする {#download-the-archive}

お使いのアーキテクチャ用のアーカイブを [GitHub Releases](https://github.com/YangTaeyoung/kiln/releases/latest) からダウンロードすることもできます。

> **注意:** Windows 版のビルドはまだコード署名されていません。インストールスクリプトによるダウンロードでは SmartScreen は表示されませんが、ブラウザーでダウンロードしたアーカイブでは表示される場合があります。**詳細情報 (More info) → 実行 (Run anyway)** を選ぶのは、Releases ページから入手したアーカイブだけにしてください。

## 初回起動 {#first-launch}

1. アプリケーションフォルダーまたはスタートメニューから **Kiln** を開きます。
2. ワークスペースのサイドバーにある **+** ボタン（macOS では `⌘N`）でワークスペースを作成します。名前を付け、プロジェクトのフォルダーを選んでください。
3. ターミナルをそのまま使うか、**新しい作業**を選んでリクエストを入力し、エージェントを選んで開始します。

フォルダーには、1 つのリポジトリも、複数のリポジトリを含む親フォルダーも指定できます。ワークスペース、作業、分割の関係については[ワークスペースと作業](workspaces.html)を参照してください。

Windows では、Kiln の実行中にもう一度 Kiln を起動すると既存のウィンドウが前面に表示され、渡したフォルダーがそのウィンドウで開きます。

## 使うツールをインストールする {#install-the-tools-you-use}

Kiln は、手元にインストールされているエージェントと GitHub のコマンドラインツールをそのまま実行します。これらのツールは同梱しておらず、サブスクリプションも含まれていません。使うツールをインストールし、ログインしておいてください。

| ツール | 用途 |
| --- | --- |
| Claude Code（`claude`） | Claude の作業 |
| Codex（`codex`） | Codex の作業 |
| Antigravity（`agy`） | Antigravity の作業。Google のインストーラーでインストールします。 |
| `gh` | GitHub の機能 |

Kiln は `agy` を `PATH`、`~/.local/bin`、`/opt/homebrew/bin`、`/usr/local/bin` から探します。Windows では、Claude Code と Codex はネイティブの Windows プログラムとして動作し、データを `%USERPROFILE%\.codex` と `%USERPROFILE%\.claude` に保存します。

Claude Code と Codex のアカウントには**設定 → アカウント**から、macOS の Antigravity には**設定 → アカウント → Antigravity** からログインできます。Windows では、ターミナルで `agy` を実行して Antigravity にログインします。詳しくは[アカウント](accounts.html)を参照してください。インストール済みの CLI のバージョンを確認したりアップデートしたりするには、**設定 → アカウント → CLI 更新**を開きます。詳しくは[会話とアップデート](conversations.html)を参照してください。

## Kiln をアップデートする {#update-kiln}

アップデートしても、実行中のターミナルセッションはそのまま保たれます。

### macOS {#macos}

Kiln は自動でアップデートを確認し、Sparkle のアップデートダイアログからインストールします。手動で確認するには、**Kiln → アップデートを確認…** を選ぶか、**設定 → このアプリについて**にあるアップデートの項目を使います。アップデートの自動確認は設定でオフにできます。

### Windows {#windows}

Kiln は 1 日に 1 回アップデートを確認します。手動で確認するには**設定 → このアプリについて → アップデートを確認…** を開き、**インストールして再起動**を選びます。Kiln は、ダウンロードしたアーカイブがリリースの署名鍵と一致することを確かめてから、ファイルを置き換えます。実行中のターミナルはそのまま動き続けます。

> **ヒント:** macOS 15 以降では、アップデート前から開いていたパネルでローカルネットワークに接続するコマンドが失敗することがあります。その場合は新しいターミナルパネルを開き、そこで再実行してください。詳しくは [macOS のローカルネットワークアクセス](terminal.html#local-network-access-on-macos)を参照してください。

## 言語を選ぶ {#choose-a-language}

**設定 → 外観 → 言語**を開き、英語、韓国語、日本語、簡体字中国語から選びます。変更はすぐに反映され、次回の起動時にも引き継がれます。

切り替わるのは Kiln 自体の画面表示だけです。ターミナルの出力、ファイル名、コード、コミットメッセージ、エージェントへのリクエストは元の内容のままです。以前のバージョンからインストールしている環境では、言語を選ぶまで韓国語で表示されます。

## カスタムテーマ {#custom-themes}

**設定 → 外観 → テーマをインポート**で JSON ファイルを選びます。有効なファイルは設定フォルダー内の **themes** にコピーされ、すぐに適用されます。選択は次回起動時も保持され、コマンドパレットでも選べます。既存のテーマファイルは上書きしません。

次の例を `ocean.json` として保存して使えます。

```json
{
  "name": "ocean",
  "label": "Ocean",
  "base": "midnight",
  "colors": {
    "bg": "#101820",
    "accent": "#80CBC4",
    "purple": "#C792EA"
  }
}
```

`name` は小文字の ASCII 英字、数字、ハイフンからなる 1〜64 文字の一意な識別子です。`label` は表示名（1〜64 文字）です。`base` は `kiln-dark`、`midnight`、`ember`、`kiln-light` のいずれかで、明暗モードと未指定の色を決めます。カスタム表示名は翻訳せずに表示します。

`colors` は省略できます。色は不透明な `#RRGGBB` 形式で指定します。使用できるキーは `bg`, `bg_panel`, `bg_elevated`, `bg_hover`, `bg_selected`, `bg_input`, `border`, `border_strong`, `text`, `text_dim`, `text_faint`, `accent`, `accent_fg`, `green`, `red`, `yellow`, `blue`, `purple`, `orange` です。ターミナルの ANSI 色を変更するには、最上位の `ansi` 配列に `#RRGGBB` 文字列を正確に 16 個指定します。黒から白までの基本 8 色、続いて明るい 8 色の順です。

**テーマフォルダーを開く**からファイルを編集・追加し、**再読み込み**で反映します。各ファイルの上限は 64 KiB です。不正なファイル、不明なフィールド、重複した識別子は **テーマファイルのエラー**に表示され、有効なテーマは引き続き使えます。保存済みテーマが見つからない場合や不正な場合は Kiln ダークを使います。選択は保持されるため、ファイルを修正して再読み込みすると復元されます。ターミナルと実行中のプロセスは終了しません。

## 次のステップ {#next-steps}

- [ワークスペースと作業](workspaces.html)で、フォルダーと作業を整理する方法を確認できます。
- [エージェント](agents.html)で、最初のエージェント作業を始められます。
- [ターミナル](terminal.html)で、シェル連携とアクティビティの報告を設定できます。
