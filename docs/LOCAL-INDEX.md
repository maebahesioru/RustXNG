# 自前インデックス

`rustxng-index` は、指定したサイトをクロールし、Tantivy の全文検索インデックスを作ります。
外部検索で見つからない文書を補う用途や、取得済みの文書だけを検索する用途に使えます。
検索できる範囲は収録したページに限られます。ページ数を増やしただけで検索精度が上がるわけではありません。

## 作成と更新

シードURLを1行ずつ書いた `seeds.txt` を用意します。

```text
https://doc.rust-lang.org/book/
https://docs.python.org/3/library/
```

```bash
rustxng-index crawl --seeds seeds.txt --out ./index-docs \
  --max-pages 5000 --max-depth 3 --max-per-host 1000 --workers 4 \
  --sitemaps --within-seed-path --contact ops@example.org

rustxng-index stats --index ./index-docs
rustxng-index search --index ./index-docs "python dict get" --limit 5
```

同じコマンドを再実行すると、今回取得したURLの文書を置き換えます。
同じURLを再登録しても文書数は増えません。再取得で `noindex`、404、410を確認した文書は削除します。
リダイレクトやcanonicalの変更時も、今回観測した要求URL・最終URLの古い記録を除きます。
過去の別canonical URLとの対応は保存していないため、未訪問の旧URLをすべて削除する機能ではありません。
取得できなかった文書や今回訪問しなかった文書は保持します。サイト全体の削除同期や変更日時による差分取得は行いません。

`--skip-indexed` は、サイトマップに載っている登録済みURLを取得せず、続きを収集するための指定です。
そのURLの更新や削除も検出しなくなるため、既存文書を更新する実行では付けないでください。
サイトマップにないシードやリンク先は、次のページを発見するために取得します。

スキーマv2の既存インデックスはそのまま使えます。それより古い索引は再作成が必要です。
読み取り専用で開く処理は、空のディレクトリや壊れた索引を自動修復・初期化しません。

## 検索サーバーから使う

StandardとCleanの組み込み設定は `index-docs` と `index-wikipedia` を参照します。
索引は配布バイナリに含まれず、そのディレクトリがなければ利用されません。
別の場所を使う場合は、設定ファイルで指定します。

```toml
[search]
local_index_path = "index-docs"
wikipedia_index_path = "index-wikipedia"
local_index_merge = true
```

`local_index_merge = true` は、検索元を指定していない一般検索に索引の結果を混ぜます。
`false` にしても、索引を明示指定した検索は利用できます。
Wikipediaの索引は、別の [`rustxng-wikidump`](WIKIPEDIA-INDEX.md) コマンドで作成します。

```bash
rustxng --profile clean --config local-settings.toml

# 自前クロールの索引だけを検索する
curl --fail --get 'http://127.0.0.1:29483/search' \
  --data-urlencode 'q=std::sync::Arc' \
  --data 'format=json' --data 'engines=local_index' --data 'max_results=10'

# 対象ドメインの中から上位の文書を探す
curl --fail --get 'http://127.0.0.1:29483/search' \
  --data-urlencode 'q=async runtime' \
  --data 'format=json' --data 'engines=local_index' \
  --data 'include_domains=doc.rust-lang.org'
```

`engines=local_index,wikipedia_index` で両方を選べます。クエリ内の `!local_index`、`!wikipedia_index` も使えます。
明示した索引がどれも利用できない場合はエラーになり、外部検索へ切り替わりません。
通常の検索元を指定した場合も、指定外のローカル索引や公式サイト検索を追加しません。
`include_raw_content` など、検索後に本文を取得する機能は別途ネットワークを使います。

ローカル索引は1ページ20件で、`pageno` により次のページへ進めます。
`max_results` は返す件数の上限で、ページの区切りは変えません。
取得可能な検索範囲は先頭10,000件までです。範囲超過や索引検索の失敗は応答に記録します。
クエリは16,384バイト・128内容語まで、ドメイン条件は許可と除外を合わせて64件までです。

## 一致と順位

タイトル・本文・URLの語をBM25で検索し、タイトル一致を3倍、URL一致を1.5倍にします。
英語などの語は、短いクエリでは全語、3語以上では内容語の約3/5の一致を必要とします。
タイトル・本文にクエリ全体がフレーズとして現れる場合は加点します。
`std::sync::Arc` のような句読点を含む語も、索引内部のフィールド指定として解釈せず検索します。

日本語・中国語は文字バイグラムを使います。同じ文字列の一部分だけが一致する文書で結果を埋めないよう、
語ごとの一致数を確認し、英語と日本語が混在するクエリにも同じ内容語の条件を適用します。
Wikipedia索引は記事の冒頭のみを持つため、従来どおり内容語のいずれかが一致すれば候補になります。

ドメインの許可・除外は、索引から上位件数を取り出す前に適用します。
`example.org` はそのホストとサブドメインに一致し、`example.org.evil.test` には一致しません。
結果のスニペットは保存した冒頭部分です。質問に合う本文抜粋は、検索後のMCP `read_page(query=...)` で取得できます。

## クロールの制約

| 項目 | 動作 |
| --- | --- |
| 接続先 | 公開HTTP(S)のみ。DNS応答を検査し、検査したアドレスへ接続する。非公開IPを含む応答は拒否 |
| プロキシ | 接続先の検証を保証できないため、プロキシ設定がある環境ではクロールを開始しない |
| リダイレクト | 最大5回。同じサイト名（wwwの有無を許容）に限定。明示ポートの変更とHTTPS→HTTPを拒否し、各移動先でrobotsと範囲を再確認 |
| robots.txt | オリジンごとに取得。通信・読み取り失敗、上限超過、429、5xxでは取得を見送る |
| 取得間隔 | `--delay-ms` は既定1,000ms。ページ・robots・サイトマップで同じホストの送信間隔を共有 |
| ページの指示 | `noindex`、`nofollow`、`X-Robots-Tag`、リンクの `nofollow`・`ugc`・`sponsored` を考慮 |
| サイトマップ | XMLとして解析し、名前空間・文字参照を処理。訪問済みURL、深さ、文書数を制限 |
| 件数 | `--max-pages` は既定500。`--max-per-host` は既定2,000 |
| サイトマップ数 | `--max-sitemaps` は既定64文書。シードのオリジンごとの上限。各文書のリダイレクトは別途最大5回。0で探索しない |
| 待機キュー | `--max-queue` は既定10,000。満杯のときに追加できなかったURL数を終了時に表示 |
| 応答サイズ | ページ2MiB、robots 512KiB、サイトマップ10MiBまで。展開後の本文に適用 |
| 終了 | ページ上限や書き込み失敗でワーカーを停止し、稼働中のワーカーが終了してから戻る |

`--within-seed-path` は、シードのディレクトリ配下に取得先を制限します。
たとえば `https://docs.python.org/3/` なら `/3/` の下を対象にします。
このパス制限はページに適用し、robots.txtやサイトマップ自体には適用しません。
canonicalは同一オリジンのURLのみ採用し、フラグメントと既知の追跡パラメータを除きます。
PDF・画像・スクリプトなどは索引クローラーの対象外です。HTML本文はJavaScriptを実行せず抽出します。
停止時はホスト間隔とバックオフの待機を解除します。送信済みのHTTP要求は完了または20秒のタイムアウトを待ちます。

## 過去の評価

今回のローカル検索・集約処理の変更前後比較は [評価手順と生データ](../bench/local-retrieval/README.md) を参照してください。
開発用の人工データによる回帰評価で、Web検索全体やExaとの品質比較ではありません。

[2026-11-04の構成比較](COMPARISON-2026-11-04.md)は、文書サイトから作った41,144ページの索引とWikipedia索引を使った記録です。
古い小規模索引の測定は [`bench/eval-100-localmerge.json`](../bench/eval-100-localmerge.json)、
127,200ページの測定は [`bench/eval-100-127k.json`](../bench/eval-100-127k.json) に残しています。
これらは異なる収録範囲・プロバイダー設定での過去の観測で、現在の変更による改善量やExaとの同条件比較を示すものではありません。
