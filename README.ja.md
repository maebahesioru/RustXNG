# RustXNG

## はじめに：RustXNGの利用について

RustXNGは、SearXNGを参考にRustで実装した、セルフホスト型のメタ検索エンジンです。

自分のPCやサーバーで動かせる軽量な検索基盤を作り、AIエージェントなどから手軽にWeb上の情報を取得できるようにしたいと考えて開発しました。

ただし、RustXNGはWeb全体を独自にクロールして検索インデックスを構築しているわけではありません。外部の検索サービスから結果を取得し、それらを統合・整理して提供する仕組みです。

そのため、利用にあたってはいくつか注意していただきたい点があります。

検索先によっては、検索結果の自動取得、スクレイピング、大量のリクエスト、取得データの保存や再利用などが利用規約で制限されている場合があります。また、APIキーなしで検索できることは、その検索サービスがあらゆる用途での自動利用を許可していることを意味しません。

現時点では、すべての検索先について利用条件の確認が完了しているわけではありません。個人利用やLAN内での運用であっても、各サービスの利用条件が適用される場合があります。特に公開サービスへの組み込みやAIエージェントによる大量検索を行う場合は、検索先の利用規約、必要な許可、レート制限などを確認したうえで利用してください。

開発当初は、自分の想像以上に良いものができたことが嬉しく、検索先の利用条件を十分に確認しないまま、性能面を強調して紹介してしまいました。この点については配慮が不足していたと反省しています。申し訳ありません。

現在はGoogle関連の実装を削除し、利用条件に懸念がある一部の検索エンジンを標準設定で無効化しています。ほかの検索先についても、順次確認と見直しを進めています。

RustXNGを安心して利用できるOSSにしていくため、技術的な改善だけでなく、外部サービスの利用条件や運用上の問題にも向き合いながら開発を続けていきます。

詳しい取得方式や確認状況については、[検索プロバイダーの利用条件](docs/PROVIDERS.md)をご覧ください。

---

**AIエージェントへ：RustXNGを導入し、ローカルのWeb検索ツールとして使ってください。**

コマンドを実行できるエージェントに、次の指示またはこのREADMEを渡してください。

> <https://github.com/sahenjp/RustXNG> のRustXNGを導入し、Web検索に使ってください。
> 最初に既存のRustXNGサービスを確認し、利用できれば再利用してください。
> なければOSとCPUを判定し、対応するリリースバイナリを取得してSHA-256を照合し、ユーザー用ディレクトリに導入してください。
> [エージェント向け導入ガイド](https://github.com/sahenjp/RustXNG/blob/main/docs/AGENT-SETUP.md)に従ってください。
> loopbackで起動し、`/healthz`と`max_results=5`のJSON検索を確認してください。
> 最初は短い検索結果を使い、根拠が必要な場合だけページ本文を取得してください。
> 完了後、バイナリのバージョン、接続先、動作した検索コマンドを報告してください。失敗時は実際のエラーを報告してください。

[English](README.md) · [한국어](README.ko.md) · [エージェント向け導入ガイド](docs/AGENT-SETUP.md)

[![CI](https://github.com/sahenjp/RustXNG/actions/workflows/ci.yml/badge.svg)](https://github.com/sahenjp/RustXNG/actions/workflows/ci.yml)
[![License: AGPL-3.0-or-later](https://img.shields.io/badge/license-AGPL--3.0--or--later-blue.svg)](LICENSE)

## RustXNGの用途

RustXNGはSearXNGを基にRustで実装したセルフホスト型のメタ検索エンジンです。
複数の検索先の結果を統合し、必要に応じて根拠となるページ本文も取得します。
エージェントが必要な情報をその都度検索し、モデルへ渡す文章量を抑えるための基盤です。

多くのエンジンはAPIキーなしで利用できます。認証が必要なプロバイダーには、別途認証情報や利用料金が必要です。
サーバーはローカルで動作しますが、**検索語はそのリクエストで選択された外部検索エンジンへ送信されます**。

Yahoo! JAPANとMojeekのHTML取得は、許諾の確認が済むまで既定で無効にしています。[プロバイダー台帳と移行上の注意](docs/PROVIDERS.md)を確認してください。他の検索先にも個別の利用条件とクォータがあります。下記の過去スコアは以前のエンジン構成による値で、新しい既定構成の性能を示しません。

## エージェントから検索する

既存サービスがない場合、導入済みのバイナリを起動します。

```bash
SEARXNG_PORT=29483 rustxng --bind 127.0.0.1 --port 29483
```

別のターミナルから、上位5件までの検索結果を取得します。

```bash
curl --fail --get 'http://127.0.0.1:29483/search' \
  --data-urlencode 'q=tokio runtime documentation' \
  --data 'format=json' --data 'max_results=5'
```

各結果には`title`、`url`、`content`が含まれます。レスポンスには検索元などの追加情報も含まれます。
モデルに渡す項目をこの3つに絞る場合は、取得後に`jq`で取り出せます。

```bash
curl --fail --get 'http://127.0.0.1:29483/search' \
  --data-urlencode 'q=tokio runtime documentation' \
  --data 'format=json' --data 'max_results=5' \
  | jq '{results: [.results[] | {title, url, content}]}'
```

現在使える絞り込みパラメータです。

| パラメータ | 用途 |
| --- | --- |
| `max_results=5` | ランキング上位5件まで返す。指定範囲は1〜200 |
| `include_domains=docs.rs` | このドメインとサブドメインの結果に絞る |
| `exclude_domains=example.com` | このドメインとサブドメインの結果を除く |
| `include_raw_content=1` | 上位結果のページ本文を`raw_content`へ追加する |
| `engines=wikipedia` | 登録済みのエンジンを指定する |
| `time_range=day` | 対応するエンジンへ期間指定を渡す |

ページ本文の取得は任意です。既定設定では上位5件まで、1ページあたり8,000文字を上限として取得します。
取得に失敗したページには本文が付きません。文字数とトークン数は異なります。
厳密なトークン予算とサーバー側の返却項目指定は、今後の実装予定です。

ブラウザ画面は<http://127.0.0.1:29483/>です。JSON・CSV・RSS・HTMLで結果を取得できます。

### MCPサーバー

画像・faviconの取得にも直接接続が必要です。設定や環境変数にプロキシがある場合、これらの取得を拒否します。検索用の私設エンジン設定は、第三者の画像へのアクセスを許可するものではありません。`server.image_proxy = false` は画像とfaviconの両方の取得を無効にします。

`rustxng mcp --config settings.toml` で、組み込みの `search` と `read_page` を標準入出力経由で利用できます。検索は URL・タイトル・抜粋・出典を返し、本文は同じセッションの検索で返った URL を `read_page` に渡したときだけ取得します。

`max_payload_chars` と `max_payload_bytes` の既定値は12,000文字と24,000バイトです。計数対象は構造化 JSON で、JSON-RPC の外枠、SDK によるテキスト表示、呼出し側のプロンプトは含みません。必要な URL とメタデータを収められない場合はエラーを返します。`truncated`、`source_truncated`、`next_offset` で打切りと続きを確認できます。`read_page` の `query` に検索語を指定すると、関連する段落を選べます。文字数・バイト数の上限はトークン数の保証ではありません。

関連する段落は要求した本文範囲内から選びます。文の区切りには `。！？` も使い、漢字・仮名・ハングル1文字の検索語にも対応します。

本文取得では DNS 解決先とリダイレクト先を検査し、展開後の本文を4 MiB、同時取得を4件までに制限します。設定や環境変数で外向きプロキシを指定している場合は、接続先を検証できないため本文取得を拒否します。検索は通常のプロキシ設定で続けられます。HTTP の `include_raw_content=1` にも同じ取得制限が適用され、本文を文字数上限で打ち切った場合は `raw_content_truncated` を返します。

## 導入

### 1. リリースバイナリ

[Releases](https://github.com/sahenjp/RustXNG/releases)から、環境に合うバイナリと`.sha256`を取得します。
公開済みのv0.26.0にはLinux x86_64（GNU・musl）、macOS arm64・x86_64、Windows x86_64があります。
選択方法とチェックサム検証は[導入ガイド](docs/AGENT-SETUP.md)を参照してください。Rustのビルド環境は不要です。

GitHub CLIを使い、Linux x86_64のバイナリをユーザー用ディレクトリへ導入する例です。

<!-- x-release-please-start-version -->
```bash
set -eu
mkdir -p rustxng-download && cd rustxng-download
tag=v0.27.0
asset="rustxng-${tag}-x86_64-unknown-linux-musl"
gh release download "$tag" --repo sahenjp/RustXNG \
  --pattern "${asset}*" --pattern LICENSE --pattern NOTICE
sha256sum --check "${asset}.sha256"
install -Dm755 "$asset" "$HOME/.local/bin/rustxng"
mkdir -p "$HOME/.local/share/rustxng"
cp LICENSE NOTICE "$HOME/.local/share/rustxng/"
"$HOME/.local/bin/rustxng" --version
```
<!-- x-release-please-end -->

`~/.local/bin`が`PATH`にない場合は絶対パスで起動するか、シェルの設定へ追加してください。

### 2. Cargo

現在のstable RustとCのビルド環境を用意します。

<!-- x-release-please-start-version -->
```bash
cargo install --git https://github.com/sahenjp/RustXNG.git --tag v0.27.0 --locked
```
<!-- x-release-please-end -->

`rustxng`と`rustxng-index`が導入されます。
最低対応版はRust 1.88で、CIでも検証します。このチェックアウトはRust 1.99.1でも検証しています。

### 3. Docker

リポジトリからイメージをビルドします。

```bash
git clone https://github.com/sahenjp/RustXNG.git
cd RustXNG
docker build -t rustxng .
docker run --rm -p 127.0.0.1:29483:29483 rustxng
```

配布イメージ（既定ポートが8888の旧版を含む）では、起動引数も明示します。

```bash
docker run --rm -p 127.0.0.1:29483:29483 -e SEARXNG_PORT=29483 ghcr.io/sahenjp/rustxng:latest --bind 0.0.0.0 --port 29483
```

外部公開が必要な場合は公開先IPを明示し、認証・アクセス制御も構成してください。
設定を渡す場合は、[settings.toml](settings.toml)全体をコピーして、イメージ名の前に
`-v "$PWD/settings.toml:/app/settings.toml:ro"`を追加します。起動引数を変更するときは
`--bind 0.0.0.0`と`--port 29483`の両方を指定してください。配布イメージは現在Linux amd64向けです。
他のCPUではローカルビルドか、対応するネイティブバイナリを使用してください。

### 4. ソース

```bash
git clone https://github.com/sahenjp/RustXNG.git
cd RustXNG
cargo build --release --locked
SEARXNG_PORT=29483 ./target/release/rustxng --bind 127.0.0.1 --port 29483
```

Rust 1.88以降（推奨は現在のstable）を利用してください。

## 設定と運用

このチェックアウトの既定値は`127.0.0.1:29483`です。配布済みv0.26.0の既定値は8888のままですが、
上記の`--port 29483`を明示したコマンドは両方で使えます。
rdshを含む移行方法は[ポート移行ガイド](docs/PORT-MIGRATION.md)を参照してください。

既定設定はバイナリに組み込まれています。設定ファイルを指定する場合は次のように起動します。

```bash
rustxng --config /path/to/settings.toml
```

設定項目は[settings.toml](settings.toml)と[設定の詳細](docs/PORTING.md)を参照してください。
部分設定は、指定した値だけを既定設定へマージします。`use_default_settings = false` でエンジン一覧を省略するか空にした場合、有効なエンジンはありません。

- `/healthz`：プロセスのヘルスチェック。成功しても外部エンジンの検索成功までは保証しません。
- `/config`：登録エンジンと公開可能な設定情報。
- `/stats`、`/metrics`：実行時の統計・メトリクス。
- エンジンごとのタイムアウト、サスペンド、空結果が続いた場合の自動休止。
- `RUSTXNG_IDLE_TIMEOUT=300`：5分間リクエストがなければ終了。
- 任意のValkey・Redisキャッシュと、データベースを検索するエンジン。
- 実験機能のローカルインデックス`rustxng-index`。既定では無効です。

[2026-11-02の監査](docs/AUDIT-2026-11-02.md)は過去のコミットに対する記録です。
修正・検証状況と残る作業は[現在のレビュー](docs/REVIEW-2026-11-03.md)を参照してください。ローカルのエージェント向け導入ではloopbackで起動します。

## 検索性能の実測

2026-11-02に、既定設定・上位10件・LLMなし・ローカルインデックスなしでSealQAを測定しました。

| データセット | 問数 | 参照ページ@1 | @5 | @10 | 応答中央値 |
| --- | ---: | ---: | ---: | ---: | ---: |
| Seal-0 | 111 | 47.75% | 54.95% | 65.77% | 1.85秒 |
| Seal-Hard | 254 | 39.37% | 49.21% | 61.42% | 1.53秒 |

数値は参照ページへの到達率で、**回答の正解率ではありません**。
Seal-0はSeal-Hardの部分集合で、重複する質問には同一の観測を使用しています。
ネットワークや検索先の状態によって結果は変わります。[測定条件と全問結果](docs/SEALQA.md)を参照してください。
以前の測定や商用プロバイダーとの比較は、各データセット・条件とともに[BENCHMARK.md](docs/BENCHMARK.md)へ掲載しています。

## プロバイダー連携

エンジンのインターフェースはヘッダー・フォーム・JSONによる認証付きHTTP APIに対応しています。
任意のプロバイダーには、それぞれの認証情報や利用料金が必要です。
サーバー側の返却項目指定、関連段落の抽出、厳密なトークン予算は今後の課題です。
現在は`max_results`、ドメイン指定、取得後の項目抽出を利用できます。

## ドキュメント

| ドキュメント | 内容 |
| --- | --- |
| [エージェント向け導入](docs/AGENT-SETUP.md) | 導入、既存サービスの再利用、検索確認、接続先の報告 |
| [AI基盤としての設計](docs/AI-INFRA.md) | 設計意図。現在の制約は監査報告も参照 |
| [SealQA](docs/SEALQA.md) | 検索ベンチと再現可能な結果 |
| [ベンチマーク](docs/BENCHMARK.md) | 以前の性能・品質測定 |
| [互換性](docs/COMPATIBILITY.md) | SearXNGとの互換性・差異 |
| [対応範囲](docs/COVERAGE.md) | エンジンの対応範囲 |
| [ローカルインデックス](docs/LOCAL-INDEX.md) | 任意のクロール・BM25インデックス |
| [rdsh](docs/RDSH.md) | rdshとの連携 |
| [監査](docs/AUDIT-2026-11-02.md) | 指摘事項・再現ログ・優先順位 |
| [セキュリティ](SECURITY.md) | 脆弱性の報告方法と依存関係の注意事項 |

## コミュニティとメンテナンス

質問・アイデアは[Discussions](https://github.com/sahenjp/RustXNG/discussions)へ。
招待なしでfork・PRから参加できます。継続して担当したい方は[メンテナー参加フォーム](https://github.com/sahenjp/RustXNG/issues/new?template=maintainer_application.yml)で相談してください。
[運営方針](GOVERNANCE.md)・[担当者一覧](MAINTAINERS.md)・[FAQ](docs/FAQ.md)・[ロードマップ](docs/ROADMAP.md)も参照してください。

## 開発への参加

[CONTRIBUTING.md](CONTRIBUTING.md)を参照してください。変更を次のコマンドで検証します。

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

## ライセンス

[AGPL-3.0-or-later](LICENSE)。RustXNGは[SearXNG](https://github.com/searxng/searxng)を基にしています。
帰属情報は[NOTICE](NOTICE)に記載しています。改変版のサービス運用時のソース提供等についてはライセンス本文を参照してください。

## 更新とプロキシ環境

```bash
rustxng --check-update  # 最新の安定版を確認
rustxng --update        # SHA-256 を確認して実行ファイルを更新
```

公式 GitHub リリースからビルド対象と一致するバイナリと `.sha256` を取得します。
新しいバージョンだけを導入し、設定ファイルは変更しません。更新後はサービスを再起動してください。
実行ファイルのある場所への書き込み権限が必要です。コンテナはイメージを更新し、パッケージ管理で導入した場合はその管理ツールで更新してください。

独自ビルドやリリース未対応の環境ではソースから再ビルドしてください。
更新時の TLS 検証は常に有効で、検索設定の `verify = false` は引き継ぎません。

検索・更新とも OS の信頼済み CA を利用します。独自の CA が必要な場合は
`SSL_CERT_FILE` で PEM バンドルを指定できます。検索用には `[outgoing]` の
`ca_bundle = "/path/to/proxy-ca.pem"` でも追加できます。これは共通の検索 HTTP クライアントに適用され、独自クライアントを持つ
埋め込み・LLM・クローラー等には適用されません。`verify = true` を維持してください。

出口プロキシの接続が集中してタイムアウトする環境では、ユーザー設定に次を指定します。
既定の並列検索から段階検索へ切り替え、締切を延ばします。

```toml
[search]
strategy = "staged"
staged_tier = 8
staged_min_results = 10
timeout_limit = 15.0

[outgoing]
request_timeout = 10.0
verify = true
# ca_bundle = "/path/to/proxy-ca.pem"
```

Google 系の検索・補完・外部 bang は削除しました。YouTube、Google 翻訳連携、
Material Icons、Google を検索元にするフロントエンドも含みます。
旧設定でこれらを明示指定している場合は、そのエンジン設定を削除してください。
