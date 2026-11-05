# プロバイダーの利用条件と未完了の確認

確認日: 2026-11-05。対象は `7b284c0` の同梱設定と実際のリクエスト実装です。
この追補は一次資料の確認結果を記録するもので、運用者の契約や個別許諾を取得した記録ではありません。
検索APIへの実リクエスト、キーの取得、有料契約、外部ベンチマークは実施していません。

取得先・認証・クォータ・保存と再配布・帰属・未完了事項は
[機械可読の台帳](provider-policy.json)に記録しました。今回の既定有効化の変更はありません。
有効108エントリのうち、[確認待ち104エントリ](provider-review-pending.json)は引き続き確認待ちです。
104は設定エントリ数であり、提供企業数ではありません。86種類のモジュールに分かれますが、
Wikipediaの3モジュールのように同じ提供元を使うものがあります。
`json_engine` や `mediawiki` は反対に複数の提供元を扱うため、モジュール名だけで許諾を共有できません。

## 今回確認した提供元

| 提供元・対象 | 一次資料から確認できた範囲 | 完了に必要な作業 |
| --- | --- | --- |
| Wikipedia 3経路・Wikidata検索 | [API方針](https://foundation.wikimedia.org/wiki/Policy:Wikimedia_Foundation_API_Usage_Guidelines)は方針に従う利用を認め、識別UA・制限通知への対応・再利用時のライセンス遵守を要求。[Wikipediaの本文条件](https://foundation.wikimedia.org/wiki/Policy:Terms_of_Use)と[Wikidataの構造化データのCC0](https://www.wikidata.org/wiki/Wikidata:Licensing)は異なる | `Retry-After` 等の待機指示への対応、総送信量、出力時の帰属とライセンス表示を確認。独自MediaWikiホストやSPARQL補完への許諾を兼ねない |
| Wiby | [公式案内](https://wiby.me/about/)はJSONによるアプリ連携を明示 | 数値クォータ、保存・再配布・帰属の条件は未確認。ソフトウェアのGPLv2を検索結果のライセンスとして扱わない |
| Stack Exchange | [API規約](https://stackoverflow.com/legal/api-terms-of-use)は提供元の明示を要求。[制限文書](https://api.stackexchange.com/docs/throttle)はメソッド共通の `backoff` と同一要求の間隔を指定 | [AUP](https://stackoverflow.com/legal/acceptable-use-policy)にはAI/ML開発・試験・索引作成・評価向け自動取得の事前書面同意の条件がある。この用途の許諾確認、メソッド単位の待機、表示側の帰属対応が未完了 |
| Qiita（既定無効） | [公式ヘルプ](https://help.qiita.com/ja/articles/qiita-api)はAPIの機能内でのアプリ作成を認める一方、広告収益化とスクレイピングを制限。[API文書](https://qiita.com/api/v2/docs)は未認証60回/時/IP、認証済み1,000回/時を記載 | 実行条件全体のリクエスト計画と残量管理、保存した記事や抜粋を公開する根拠の確認。APIキーだけでは再配布条件は解決しない |
| Marginalia（既定無効） | [2025-12-08更新のAPI案内](https://about.marginalia-search.com/article/api/)では非商用キーはCC BY-NC-SA 4.0、商用キーは別条件。`public` キーは連携開発用 | 実際のキー種別・枠・用途を選択し、返却ライセンスを保持。開発用キーを既定値にせず、独自接続先に公開サービスの条件を自動適用しない |
| Brave Search API（既定無効） | [2026-09-01版規約](https://api-dashboard.search.brave.com/documentation/resources/terms-of-service)には一時保存を超える保存・再配布・AI評価の制限がある。[公式FAQ](https://brave.com/search/api/)は保存権付きプランも案内 | 実際の契約・注文書とキーに保存・評価・公開の権利が含まれるか確認。一般規約だけで許諾済みとも、全プランで禁止とも判断しない |

API方針やライセンスの存在と、現行の実装・運用が条件を満たすことを分けて記録しています。
一次資料を追加した保留中のレコードには `standard_allowed = true` を付けていません。
エンドポイント変更や新規有効化の確認を省略する根拠には使えません。

## 実装の修正と残る制約

Stack ExchangeのURLは、APIの `link` をそのまま使います。
`api_site = "math"` から誤って `math.com` を生成していた処理を削除し、
`owner.link` と `content_license` も結果のメタデータへ保持します。
ライセンスは[投稿・改訂時期によって異なる](https://stackoverflow.com/help/licensing)ため、一律4.0にはしません。
リンクの欠落やHTTP(S)以外のリンクは結果として返しません。

Marginaliaは返却された `license` を保持し、公開APIの結果には提供元と規約URLを付けます。
古い固定UAを削除し、Qiitaとともに共通ネットワーク層の版・連絡先付きUAを使います。
これらのメタデータはJSON/MCPの利用者にも渡りますが、通常のHTML結果での完全な帰属表示や、
Stack ExchangeのJSON `backoff` 対応が完了したことを意味しません。

Wibyの台帳は設定名と `json_engine` モジュール・接続先の組で扱います。
別のJSONプロバイダーには引き継がず、既定無効としたモジュールの制約を個別レコードで上書きできないようにしました。
`docs/provider-review-pending.json` と既定の有効・無効設定は変更していません。

## 評価とIssueの完了条件

- [#63](https://github.com/sahenjp/RustXNG/issues/63): 今回は上表の提供元に証拠と未完了事項を追加。残りの提供元も同じ粒度で確認し、運用条件を満たす必要があります。
- [#16](https://github.com/sahenjp/RustXNG/issues/16): Bingの市場固定は実装済みですが、追加市場の既定有効化には取得・再利用の確認が必要です。[Bing Search APIの終了](https://learn.microsoft.com/en-us/lifecycle/announcements/bing-search-api-retirement)を回避するためにHTML経路へ許諾が移ることはありません。
- [#40](https://github.com/sahenjp/RustXNG/issues/40): 比較ツールの事前検証とリクエスト計画は[#106](https://github.com/sahenjp/RustXNG/pull/106)で扱います。テンプレート30例と既存の6問は未見評価の代替ではありません。独立して収集した実際の診断、固定した基準エンジン、権利を確認した構成、反復実測が残ります。
- [#104](https://github.com/sahenjp/RustXNG/issues/104): この追補に新しい検索品質の測定値はありません。許諾を確認した検索元または独立索引で、多言語・鮮度の未見設問を評価する必要があります。既存100問の回復やExa超えは主張しません。

外部測定では、実行日時、コード・設定・設問のハッシュ、実際のエンジン一覧、
キャッシュと索引、言語・地域、キーのプランと枠、保存可能な情報の範囲を記録します。
キーそのものは記録しません。拒否や枠の超過を迂回して測定を続けません。
