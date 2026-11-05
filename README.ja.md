# RustXNG

[English](README.md) · [한국어](README.ko.md)

RustXNGは、Rust製のセルフホスト型メタ検索エンジンです。[SearXNG](https://github.com/searxng/searxng)をもとに開発しています。複数の検索サービスから結果を集め、JSON APIやMCPを通じてAIエージェントに渡せます。

Web全体を独自に索引化する検索エンジンではありません。必要なサイトだけを`rustxng-index`でクロールし、ローカル検索の結果を加えることはできます。Wikipediaの公式ダンプから索引を作る機能もあります。

## 使い始める

[Releases](https://github.com/sahenjp/RustXNG/releases)からOSに合うバイナリを取得できます。実行前に、配布ファイルと一緒に公開されたSHA-256チェックサムを確認してください。

Cargoから導入する場合は次のコマンドを使います。

<!-- x-release-please-start-version -->
```bash
cargo install --git https://github.com/sahenjp/RustXNG.git --tag v0.28.0 --locked
```
<!-- x-release-please-end -->

ソースからの導入にはRust 1.88以降とCのビルド環境が必要です。バイナリの選び方や動作確認は[エージェント向け導入ガイド](docs/AGENT-SETUP.md)を参照してください。

ローカルでサーバーを起動します。

```bash
rustxng --bind 127.0.0.1 --port 29483
```

別のターミナルから検索できます。

```bash
curl --fail --get 'http://127.0.0.1:29483/search' \
  --data-urlencode 'q=tokio runtime documentation' \
  --data 'format=json' --data 'max_results=5'
```

結果にはタイトル、URL、抜粋、検索元などの情報が含まれます。必要に応じて`language=ja`、`engines=wikipedia`、`include_domains=docs.rs`、`exclude_domains=example.com`、`time_range=day`、`include_raw_content=1`を指定できます。期間指定の精度は検索元によって異なります。

ブラウザーからは<http://127.0.0.1:29483/>を開いてください。JSON以外にCSV、RSS、HTMLでも結果を取得できます。

## AIエージェントから使う

組み込みのMCPサーバーは次のコマンドで起動します。

```bash
rustxng mcp
```

MCPには読み取り専用のツールが2つあります。

- `search`：検索結果を順位順に返します。既定は5件、最大10件です。抜粋は1件500文字までです。
- `read_page`：同じセッションの`search`が返したURLから本文を取得します。`query`を指定すると、取得済みのページ全体から関連する文章を最大3件選びます。

`read_page`の既定の本文長は4,000文字で、`max_chars`は最大20,000まで指定できます。`offset`と`next_offset`を使えば続きも読めます。情報が省略されたかは`truncated`と`source_truncated`で確認できます。応答には文字数・バイト数の上限がありますが、トークン数を厳密に制限する機能ではありません。

本文取得では接続先とリダイレクトを検査し、プライベートIPへの接続を拒否します。robots.txt（RFC 9309）にも従います。MCPから任意のURLを直接取得することはできません。取得したWebページはAIへの指示ではなく、外部の情報として扱ってください。

HTTP APIでは`include_raw_content=1`で本文を取得できます。PDFの文字抽出にはPoppler（`pdftotext`）が必要です。スキャン画像のOCRには対応していません。JavaScriptで本文を表示するページには、別途[Firefoxレンダラー](tools/page-renderer/README.md)を設定します。

## 検索先の構成

起動時に`--profile`で検索先を切り替えられます。既定は`standard`です。

| 構成 | コマンド | 検索先 |
| --- | --- | --- |
| Standard | `rustxng` | 取得先と用途の条件を記録したキー不要の34エントリと自前索引 |
| API | `rustxng --profile api` | Standard に明示設定した有料 Exa Search を追加（`EXA_API_KEY`） |
| Clean | `rustxng --profile clean` | 選択した公開APIと自前の索引 |
| Full | `rustxng --profile full` | 利用規約の制限や許可が未確認のHTML検索先も含む構成 |

Fullは利用許可を不要にする機能ではありません。個人利用やLAN内での運用でも検索元の規約が適用されます。必要な許可や契約を確認できない検索先は有効にしないでください。Cleanも法的な適合性を保証するものではありません。

MCPでも`rustxng mcp --profile clean`のように指定できます。検索元によってはAPIキーや料金が必要です。RustXNGをローカルで動かしても、検索語は選択した外部サービスへ送信されます。

Standardでは未確認の検索元をすべて既定無効にしました。確認範囲は記録した取得用途で、原著作物のライセンス・クォータ・運用目的の条件は残ります。キーが存在するだけではAPIを有効にしません。[今回の測定と設定方法](docs/ISSUE-RESOLUTION-2026-11-05.md)を参照してください。

## 導入と設定

Dockerでも起動できます。

```bash
docker run --rm -p 127.0.0.1:29483:29483 \
  -e SEARXNG_PORT=29483 ghcr.io/sahenjp/rustxng:latest \
  --bind 0.0.0.0 --port 29483
```

既定ではローカル接続のみを受け付けます。外部へ公開するときは認証やアクセス制限を設定してください。

設定はバイナリに含まれています。変更したい項目だけをTOMLファイルに書いて指定できます。

```bash
rustxng --config settings.toml
```

設定ファイルは選択したプロファイルを上書きします。意図せず上書きしないよう、既定設定を丸ごとコピーするのは避けてください。

更新確認は`rustxng --check-update`、対応するバイナリの更新は`rustxng --update`です。パッケージ管理やDockerで導入した場合は、それぞれの方法で更新してください。

`/healthz`で確認できるのはプロセスの稼働状態です。検索元が使えるかどうかは、実際にJSON検索を実行して確認します。稼働統計は`/stats`と`/metrics`で確認できます。

## 検索精度について

[2026年11月4日の比較](docs/COMPARISON-2026-11-04.md)では、技術文書100問とSealQA 254問で3つの構成を測定しています。技術文書100問のHit@10は、Standardが88、Cleanが61、Fullが98でした。測定時にはローカルの索引を使用しています。

これは正解ページが検索結果に含まれる割合であり、AIの回答正解率ではありません。評価用の質問は開発中の調整にも使われており、索引対象には設問と関連する文書サイトが含まれます。他社APIの保存済み結果とも測定日時が異なります。この数値だけで一般的な検索性能の優劣は判断できません。

## 関連ドキュメント

- [エージェントへの導入と動作確認](docs/AGENT-SETUP.md)
- [検索元の利用条件と設定](docs/PROVIDERS.md)
- [自前クローラーと索引](docs/LOCAL-INDEX.md)・[Wikipediaの索引](docs/WIKIPEDIA-INDEX.md)
- [SearXNGとの互換性](docs/COMPATIBILITY.md)・[過去のベンチマーク](docs/BENCHMARK.md)
- [セキュリティ](SECURITY.md)・[開発への参加](CONTRIBUTING.md)・[リリース手順](docs/RELEASING.md)

## 開発への参加とライセンス

不具合の報告や提案は[Issues](https://github.com/sahenjp/RustXNG/issues)と[Discussions](https://github.com/sahenjp/RustXNG/discussions)へお願いします。PRも歓迎します。開発時の手順は[CONTRIBUTING.md](CONTRIBUTING.md)、運営方針は[GOVERNANCE.md](GOVERNANCE.md)と[MAINTAINERS.md](MAINTAINERS.md)を参照してください。

RustXNGのライセンスは[AGPL-3.0-or-later](LICENSE)です。SearXNGと外部の貢献者への帰属表示は[NOTICE](NOTICE)に記載しています。コードのライセンスは、外部検索サービスのデータ取得や再配布を無条件に許可するものではありません。
