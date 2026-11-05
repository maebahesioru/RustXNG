# 検索プロバイダーの接続方式と利用条件

RustXNGは外部プロバイダーの検索結果を統合します。RustXNGのAGPLライセンスはコードの利用条件であり、
外部サイトの自動取得、検索結果の保存・再利用・再配布を許可するものではありません。
APIキーの要否、取得方式、認証、クォータ、利用目的、再配布条件はプロバイダーごとに確認してください。
公開APIやRSSであることだけでも、無制限の自動利用を意味しません。

## 2026-11-05のStandard設定と監査

Bing（検索・ニュース・画像・動画）、Seznam、eBay、Pinterest、IMDb、Rotten Tomatoes、
Flickrの非API経路、Unsplashの内部API経路を既定無効にしました。
これらの変更は許諾の確認状況に基づく保守上の判断です。
特にSeznamとRotten Tomatoesは、禁止の断定ではなく適用範囲の確認待ちです。
Fullにも自動では追加しません。既存のユーザー設定での明示的な有効化は引き続き優先します。

[監査台帳](provider-policy.json)には取得先・方式・一次資料・個別の確認日・条件を記録します。
新たに確認した一次資料は、[eBayの利用規約](https://www.ebay.com/help/policies/member-behavior-policies/user-agreement?id=4259)、
[Pinterestの利用規約](https://policy.pinterest.com/en/terms-of-service)、
[IMDbのデータ利用案内](https://help.imdb.com/article/imdb/general-information/can-i-use-imdb-data-in-my-software/G5JTRESSHJBBHTGX)、
[Flickrの利用規約](https://www.flickr.com/help/terms)、
[Rotten Tomatoesの規約案内](https://www.rottentomatoes.com/policies/terms-and-policies)、
[Unsplashの公式API文書](https://unsplash.com/documentation)です。
[従来のBing Search APIは2025-08-11に終了](https://learn.microsoft.com/en-us/lifecycle/announcements/bing-search-api-retirement)しており、
そのAPIへの単純な置き換えは行いません。

[Nominatimの公開サーバー方針](https://operations.osmfoundation.org/policies/nominatim/)と
[MwmblのAPI規約](https://developer.mwmbl.org/terms/)に合わせ、該当ホストだけ送信間隔を1,100msに設定しました。
キャッシュヒットは送信枠を消費せず、同一リクエストの同時キャッシュミスは1回にまとめます。
他ホストの既定間隔は0msのままで、並列検索を維持します。
間隔制御と同時リクエストの集約はプロセス内の機能です。
複数プロセス・複数台で運用する場合は全体の送信量を別途調整してください。

Nominatimではハードコードした古いUAを削除し、現行バージョンと`[outgoing] contact`を使います。
結果の本文とメタデータにOpenStreetMapの帰属を含めます。
公開サーバーは少量の利用者による検索向けです。自動補完・体系的取得・汎用の再販売に使えません。
運用者自身による用途の判断と方針への対応が必要です。
`[[engines]] name = "openstreetmap"` の `base_url` に検索エンドポイント全体を指定すれば、
自前または契約済みのサービスへ切り替えられます。

Mwmblの匿名枠は月1,000回・毎秒1回です。月間枠は上流で適用され、RustXNGは残量を推定しません。
APIへのアクセス条件と返却データのCC BY-NC-SA 4.0は別です。
商用アプリからAPIに接続できても、結果の商用再利用まで許可されたとは扱えません。
保存・再配布時は帰属、非営利条件、継承条件、原著作物の権利を確認してください。
公開APIの結果には`source_license`・`source_license_url`・`source_terms_url`を付けます。
自前のMwmblエンドポイントには、この公開サービスのライセンスを自動適用しません。

Hacker NewsとEurope PMCも台帳に記録しました。
[HN Algoliaの実装とAPI案内](https://github.com/algolia/hn-search)は検索経路の資料であり、
投稿内容の無制限な再利用許諾ではありません。
[EMBL-EBIの規約](https://www.ebi.ac.uk/about/terms-of-use/)は2026-10-14改訂版を確認し、
第三者の抄録・本文の権利は別に残ることを記録しています。

CIの`python tools/check_provider_policy.py`はStandardと明示的なAPIプロファイルを検査します。
**有効34件すべてに、対象の取得先と用途を限定した確認記録があります。** 未確認・制限対象は既定無効です。
一般的なMediaWiki/JSONアダプターの確認を別サイトに流用できません。設定の取得先、最低間隔、
アダプターのソースハッシュを照合し、変更時は一次資料と実装の再確認を必要とします。
[確認待ち一覧](provider-review-pending.json)は無効な検索元の作業一覧で、例外許可ではありません。

Standardも識別できるUser-Agentを既定で使います。追加したホスト別の間隔は、
crates.io・Repology・Packagist・Wibyが1,100ms、arXiv・Openverseが3,100msです。PubMedは独自の二段階通信が共有制御を通らないため、既定無効にしました。
固定間隔はIP・アカウント単位の日次/月次枠の代わりにはなりません。429/Retry-Afterと休止も既存の共有経路で扱います。

`--profile api`はStandardにExaの文書化された有料APIを追加します。`EXA_API_KEY`か明示設定のキーが必要です。
`lite`検索、10件、500文字のhighlightsを使い、期間指定は`startPublishedDate`に変換します。
環境変数にキーがあるだけではStandardで有効になりません。料金・アカウントの枠と第三者コンテンツの権利は別です。
Open-Meteoの無料経路は非商用限定、Mwmblの返却データはCC BY-NC-SAなど、条件付きの検索元も含みます。
商用利用・保存・再配布を一括で許可された構成とは説明しません。

速度と検索品質の検証は[変更前後の測定記録](PERFORMANCE-2026-11-05.md)に分けて記載します。
2026-11-04以前の結果は当時の構成の履歴であり、新しいStandardの実測値ではありません。

## 2026-11-03の既定設定の変更

| エンジン | 実装の取得方式 | 一次資料で確認した制約 | 判断 |
| --- | --- | --- | --- |
| Yahoo! JAPAN | `search.yahoo.co.jp/search` のHTML解析 | [公式ヘルプ](https://support.yahoo-net.jp/PccSearch/s/article/H000015794)は、インデックス作成目的のクロール等の例外を除き、スクレイピング等の機械的な情報収集・利用を禁止 | 許諾未確認のため既定無効 |
| Mojeek | `www.mojeek.com/search` のHTML解析 | [Terms §3(5)](https://www.mojeek.com/about/terms.html)は認可API利用者を除く自動アクセスと、事前同意のないスクレイピングを禁止 | 許諾未確認のため既定無効 |
| Bing | `www.bing.com/search` のHTML解析 | [Microsoftサービス規約「BingおよびMSN」](https://www.microsoft.com/ja-jp/servicesagreement)には利用・再配布・独自製品での利用の制約がある | 2026-11-05に既定無効へ変更 |

これは個別利用の違法性の判断ではありません。許諾を確認できていないHTML経路を標準利用から外す保守上の判断です。
Yahoo! JAPANとMojeekのアダプターは残します。利用者が明示的に有効化する場合は、先に必要な許可・契約を確認してください。
既存のユーザー設定が `disabled = false` を明示していれば、その指定は引き続き優先されます。
既存の有効化設定も見直してください。プロキシ・UA変更やCAPTCHA回避を、利用条件の問題の解決策として扱いません。

## 2026-11-03の追加変更（監査 `RustXNG_全270プロバイダー規約・性能監査` への対応）

| 変更 | 内容 |
| --- | --- |
| NAVER・Keenable・Swisscows を既定無効 | NAVER は公表規約との整合が未確認。Keenable は API キーがなくても公開エンドポイントを呼ぶ。Swisscows は署名付きエンドポイントの第三者利用が未確認。有効化する場合は先に許可・条件を確認してください |
| API 提供元には識別できる User-Agent を送る | Wikimedia 系、GitHub、crates.io、PyPI、npm、Crossref、OpenAlex、Semantic Scholar、StackExchange、arXiv、Openverse、OSM（Nominatim・Photon）、Open-Meteo 宛ての通信は、ブラウザ風のローテーションではなく `rustxng/<版> (+https://github.com/sahenjp/RustXNG[; 連絡先])` を使う。連絡先は `[outgoing] contact` |
| `read_page` / 生本文取得 | 識別できる User-Agent を使い、robots.txt（RFC 9309）に従う。robots.txt に到達できない場合のみ、利用者が求めたページは取得を続ける |
| カテゴリ指定の誤り 22 件を修正 | `categories = ["g","e","n",...]` のように 1 文字ずつの配列になっていた。20 件は設定上有効だったが、実際にはどのカテゴリにも属していなかった。**修正後も、これまで一度も動いていないことを踏まえて 20 件とも既定無効にした**（有効化は各プロバイダーの確認後） |
| 比較表現の修正 | `settings.toml` のコメントから他社サービスとの優劣を示す記述を除いた |
| クリーン構成 | 公開APIの利用方針を確認しやすい経路と自前クロールに絞った `settings.clean.toml` を追加（`docs/CLEAN-PROFILE.md`）。公開APIであっても用途・再利用の許諾は別途確認が必要 |

プロキシ・UA 変更や CAPTCHA 回避を、利用条件の問題の解決策として扱わない方針は変わりません。

## 2026-11-04の既定設定の変更（通常構成の強化）

既定の設定で 10 問を流し、各エンジンの応答を確認しました。そのうえで次のとおり変更しました。どれも API キーは不要です。

| 変更 | 対象 | 理由 |
| --- | --- | --- |
| 既定無効 | 米 Yahoo | [Yahoo 利用規約 2.4(9)](https://legal.yahoo.com/us/en/yahoo/terms/otos/index.html) は、事前の明示的な許可のない自動手段（ロボット・スクレイパー等）によるデータ取得を禁止 |
| 既定無効 | Brave、DuckDuckGo（2 件）、Yandex、Qwant、Dogpile、AndiSearch、Findborg、Fireball、NeoSearch、TuskSearch、searchmysite、Quark | 最初の問い合わせから CAPTCHA・アクセス拒否・429 を返した。サイト側が自動アクセスを拒んでいるので、送り続けない。結果も返らない |
| 既定無効 | Erowid、Luxxle、Sogou、Yep、Neocities | すべての問い合わせで失敗し、毎回タイムアウトまで検索を待たせていた |
| 既定無効 | Grokipedia | 技術文書 100 問・SealQA 254 問のどちらでも正解を 1 件も返さず、上位の枠だけを使っていた |
| 一般検索から外す | Wikibooks・Wikiquote・Wikisource・Wikispecies・Wikiversity・Wikivoyage | 一般検索の正解を 1 件も返さず、上位 10 件の枠を使っていた。上流の SearXNG でも既定無効。bang（`!wb` など）と `wikimedia` カテゴリでは引き続き使える |
| 自動補完 | DuckDuckGo → Wikipedia（opensearch API） | DuckDuckGo はボットに CAPTCHA を返す。Wikipedia は公開 API |
| Wikipedia ダンプの索引 | `wikipedia_index_path = "index-wikipedia"` | 外部への問い合わせは増えない。ディレクトリが無ければ使わない（`docs/WIKIPEDIA-INDEX.md`） |
| Mwmbl | v1 → 公式 v2 API | [Mwmbl API 規約](https://developer.mwmbl.org/terms/)は、キーなしの匿名利用（1 req/s・月 1,000 回）と商用利用を認める。結果は CC BY-NC-SA 4.0。キーがあれば `api_key` で枠が増える |
| ローカル索引の検索 | 締め切りで打ち切る | 索引の読み込みが遅いとき、検索全体を止めない |
| 自前の文書索引 | `local_index_path = "index-docs"` | 自前クローラーで作る（`docs/LOCAL-INDEX.md`）。ディレクトリが無ければ使わない |
| 公式サイトの補完 | Wikidata Query Service | 見つかった Wikipedia 記事の公式サイト・リポジトリ・arXiv を 1 回の SPARQL で引く（`src/official.rs`） |

この時点では Seznam と Bing を有効のまま残しましたが、2026-11-05の変更で既定無効にしました。
**現在のStandardは取得用途を限定した確認記録のある検索元だけを有効にします。** この記録は、すべての用途や原著作物の再利用を許可するものではありません。

Hacker News（Algolia）と Europe PMC も、自動利用についての規定を確認できませんでした。HN の API ページは規約を載せておらず、Europe PMC の規約ページは 403 を返しました。EMBL-EBI の一般規約は、他の利用者を妨げない範囲での利用を認めています。

**測定**（SealQA 254 問、URL 完全一致、直列 2 秒間隔、1 回、自前索引あり）: hit@1 70、hit@5 116、**hit@10 132（52.0%）**、MRR@10 0.352、エラー 0、中央値 1.4 s。
クリーン構成は hit@10 123、フル構成（`settings.full.toml`）は 165 です。技術文書 100 問と Exa・Tavily との比較は `docs/COMPARISON-2026-11-04.md` を参照してください。旧監査で、保存済みの結果から Swisscows と Yahoo! JAPAN を除いて試算した値は hit@10 57 でした（新しい検索ではないため、直接は比べられません）。
正解が Wikipedia 以外の 58 問で取れたのは 2 問です。

## 追加許諾が必要な検索先を含む構成（`settings.full.toml`）

既定の設定に、禁止・制限・許諾未確認のため標準で無効にしている検索先を追加する構成です。**個人・家庭内・非公開の運用でも、各提供元の利用規約は免除されません。** 各検索先で想定する自動取得・再利用が明示的に許可されている場合に限り有効化してください。禁止されている取得方式は、適切な個別許諾なしに使わないでください。公開インスタンスへの組み込みは推奨しません。

| 有効にするもの | 既定で無効にしている理由 |
| --- | --- |
| Yahoo（米国） | 利用規約 2.4(9) が許可のない自動アクセスを禁止 |
| Yahoo! JAPAN | 公式ヘルプが、インデックス作成目的のクロール等の例外を除きスクレイピングを禁止 |
| NAVER | 公表規約との整合が未確認 |
| Keenable | API キーなしで公開エンドポイントを呼ぶ |
| fynd | 規約未確認 |

2026-11-04 に、既定で無効の一般検索 58 件を 1 件ずつ 5 問で試しました。上の 5 件のほかに結果を返したのは reloado（数分で 429）、ayo・Quark（たびたびタイムアウト）、bing Japan（Bing と同じ結果）です。どれも速度か重複の理由で入れていません。DuckDuckGo、Brave、Mojeek、Qwant、Yandex などは最初の問い合わせから CAPTCHA・アクセス拒否・429 を返すので、この構成でも無効のままです。**RustXNG はこうした拒否を回避しません**（UA の偽装・CAPTCHA の突破・プロキシの切り替えはしません）。

比較は `docs/COMPARISON-2026-11-04.md` を参照してください。

## DeepSeek Search / Fetch API

`deepseek` エンジンは DeepSeek の Search API を検索先として使います。APIキーが必要で、既定では無効です。Search と Fetch は成功したリクエストごとに課金され、それぞれ QPS の上限があります。料金・上限・データの扱いは [DeepSeek の検索ガイド](https://api-docs.deepseek.com/guides/search)で確認してください。

本文取得を DeepSeek Fetch API に切り替える場合は、`settings.toml` の `[fetch]` で `provider = "deepseek"` を指定します。この場合、`read_page` と `include_raw_content` が取得対象の URL を DeepSeek へ送信します。既定の `direct` は、宛先をローカルで検証する従来の取得のままです。

## 全設定エントリの台帳

[機械可読CSV](providers.csv)は同梱設定の全273エントリ（設定上有効34、無効239）を列挙します。[一次資料の確認記録](provider-policy.json)から、
接続方式・規約URL・確認日・判断を載せます。**273件中150件は `unreviewed` のまま**で、すべて既定無効です。その他の無効な検索元には制限・登録条件などの確認記録があります。有効34件の条件付き確認と、無効な検索元の調査記録は区別します。
`clean` の許可リストに載っていても、台帳上の確認状態が自動的に更新されるわけではありません。公開APIの存在と、RustXNGでの自動取得・保存・再配布についての許諾を区別してください。全件の監査が済んだわけではありません。

`enabled_in_config` は `Settings::enabled_engines` と同じ設定上の条件です。初期化成功・認証の充足・
そのリクエストのカテゴリ選択・実際の応答を保証しません。設定エントリ数と登録済みエンジン数も区別してください。
`source_url_literals_unverified` はテスト部分を除くソース内URL文字列の候補です。説明用URLやテンプレートも含み、
実際の接続先・取得方式として確認済みの値ではありません。未確認の接続先は実装を読んで台帳へ追記します。

```bash
python3 tools/provider_inventory.py
python3 tools/provider_inventory.py --check
```

今後は有効な一般検索から順に、リクエスト先、HTML/API/RSS等の取得方式、認証、レート制限、保存・再配布条件、
規約の確認日、利用者の契約範囲を確認します。必要な許可が確認できるAPI等への置換も個別に検討します。

## 評価と移行

旧SealQAでは統合後の1位結果の代表エンジンが189/254問でYahoo! JAPANでした。この件数は、
Yahoo! JAPANを外した場合の性能低下量を表すものではありません。
**その後、2026-11-04に`standard`・`clean`・`full`で再測定しました**（[構成別の比較・条件・限界](COMPARISON-2026-11-04.md)）。同じ254問のhit@10は、`standard`が128〜132、`clean`が123、`full`が165です。どの数値も検索による参考URL到達率で、回答精度や利用許諾の保証ではありません。**保存メタデータのコミットはv0.28.0のリリースコミットと一致していない**ため、リリースバイナリの再測定値とも断定しません。

新しい比較では、未見の設問、取得時刻、キャッシュ・索引の有無、プロバイダーの利用条件を記録して検証します。
元の観測は履歴として保存し、[URL照合を修正した再集計](BENCHMARK-RESCORE-2026-11-03.md)も別に公開します。
