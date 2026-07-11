export const supportedLocales = ["en", "ja"] as const;
export type AppLocale = (typeof supportedLocales)[number];

type TranslationValues = Record<string, string | number>;

export const resolveSupportedLocale = (
  languages: readonly (string | undefined)[],
): AppLocale => {
  for (const candidate of languages) {
    const language = candidate?.trim().toLowerCase();
    if (!language) continue;
    if (language === "ja" || language.startsWith("ja-")) return "ja";
    if (language === "en" || language.startsWith("en-")) return "en";
  }
  return "en";
};

const detectLocale = (): AppLocale =>
  resolveSupportedLocale([
    ...(typeof navigator !== "undefined" ? navigator.languages : []),
    typeof navigator !== "undefined" ? navigator.language : undefined,
    Intl.DateTimeFormat().resolvedOptions().locale,
  ]);

export let appLocale = detectLocale();
const localeListeners = new Set<() => void>();

export function setAppLocale(locale: AppLocale) {
  if (locale === appLocale) return;
  appLocale = locale;
  for (const listener of localeListeners) listener();
}

export function subscribeAppLocale(listener: () => void) {
  localeListeners.add(listener);
  return () => localeListeners.delete(listener);
}

export function getAppLocale() {
  return appLocale;
}

const jaMessages = {
  "timeline.home": "ホーム",
  "timeline.public": "連合",
  "timeline.local": "ローカル",
  "timeline.notification": "通知",
  "timeline.bookmarks": "ブックマーク",
  "timeline.favourites": "お気に入り",
  "timeline.hashtag": "ハッシュタグ",
  "timeline.list": "リスト",
  "timeline.custom": "カスタム",
  "timeline.yq": "YQ",
  "timeline.search": "検索",
  "timeline.userBookmarks": "ブックマーク",
  "timeline.thread": "スレッド",
  "timeline.profile": "プロフィール",
  "timeline.airContext": "AIR context",
  "timeline.unknown": "不明なタイムライン ({type})",
  "timeline.unsupported": "このタイムライン種類 ({type}) はこのバージョンでは利用できません",
  "timeline.empty": "読み込まれた投稿はありません。",
  "a11y.menu": "メニュー",
  "a11y.dialog.close": "ダイアログを閉じる",
  "a11y.media.moved": "メディアを {position} 番目へ移動しました",
  "settings.section.account": "アカウント",
  "settings.section.appearance": "外観",
  "settings.section.behavior": "動作",
  "settings.section.performance": "パフォーマンス",
  "settings.section.notification": "通知",
  "settings.section.timeline": "タイムライン",
  "settings.section.sidecar": "サイドカー",
  "settings.section.database": "データベース",
  "settings.section.debug": "デバッグ",
  "settings.section.about": "このアプリについて",
  "Account changed during an operation": "操作中にアカウントが変更されました",
  Cancelled: "キャンセルしました",
  Completed: "完了しました",
  "Delete all cached statuses? This cannot be undone.":
    "キャッシュされた投稿をすべて削除しますか？この操作は元に戻せません。",
  "Log out {acct}?": "{acct} からログアウトしますか？",
  "No active account is signed in": "ログイン中のアカウントがありません",
  "Operation failed": "操作に失敗しました",
  "Run database maintenance now?": "データベースのメンテナンスを実行しますか？",
  "Saving settings": "設定を保存しています",
  "Saving status action": "投稿への操作を保存しています",
  "Settings changed while switching accounts":
    "アカウント切り替え中に設定が変更されました",
  "Settings could not be saved": "設定を保存できませんでした",
  "Settings saved": "設定を保存しました",
  "Status action failed and was rolled back":
    "投稿への操作に失敗したため元に戻しました",
  "Status action result is uncertain": "投稿への操作結果を確認できません",
  "Starting background services": "バックグラウンドサービスを起動中",
  "Startup initialization failed": "起動時の初期化に失敗しました",
  "Updating the local database. Large caches can take several minutes.":
    "ローカルデータベースを更新しています。キャッシュが大きい場合は数分かかることがあります。",
  "The result is uncertain. Refresh before retrying.":
    "操作結果を確認できません。再試行する前に更新してください。",
  "The status action result is uncertain": "投稿への操作結果を確認できません",
  "This action is not supported by the selected account":
    "選択中のアカウントではこの操作を利用できません",
  "Vacuum database": "データベースを Vacuum",
  "Waiting for confirmation": "確認を待っています",
  Working: "処理中",
  unread: "未読",
  Account: "アカウント",
  Accounts: "アカウント",
  Activate: "有効化",
  Active: "有効",
  Activities: "アクティビティ",
  "Add Account": "アカウントを追加",
  "Add Pane": "ペインを追加",
  "Add Tab": "タブを追加",
  "Add option": "選択肢を追加",
  AlwaysExpand: "常に展開",
  AlwaysShow: "常に表示",
  Animals: "動物",
  "Animals & Nature": "動物と自然",
  About: "このアプリについて",
  Appearance: "外観",
  "API Rate Limit": "API レート制限",
  "AIR context": "AIR context",
  "AIR context target is invalid": "AIR context の対象が不正です",
  "AIR context target is missing": "AIR context の対象がありません",
  "Application uptime": "アプリケーション稼働時間",
  "Authentication expired. Please sign in again.":
    "認証の有効期限が切れました。もう一度ログインしてください。",
  "App password": "アプリパスワード",
  "Awayuki could not start": "Awayuki を起動できませんでした",
  "Awayuki could not restore its local data and accounts.":
    "ローカルデータとアカウントを復元できませんでした。",
  "Awayuki encountered an unexpected UI error":
    "Awayuki の UI で予期しないエラーが発生しました",
  "Attach media": "メディアを添付",
  "Add preset": "プリセットを追加",
  "Avatar shape": "アバター形状",
  "Auto visibility applied": "自動 visibility が適用されています",
  "Automatically switch visibility when the post text contains a keyword. The first matching preset is applied.":
    "投稿本文にキーワードが含まれる場合、自動的に visibility を変更します。最初に一致したプリセットが適用されます。",
  Back: "戻る",
  Behavior: "動作",
  Block: "ブロック",
  "Block {acct}": "{acct} をブロック",
  Bookmark: "ブックマーク",
  Bookmarks: "ブックマーク",
  "Bookmarks by {acct}": "{acct} のブックマーク",
  Boost: "ブースト",
  "Boost this post by {subject}?": "{subject} の投稿をブーストしますか？",
  "Bluesky fetch interval": "Bluesky 取得間隔",
  "Bluesky login": "Bluesky にログイン",
  Cancel: "キャンセル",
  "Clear target post": "対象投稿をクリア",
  "Clear Status Cache": "投稿キャッシュを削除",
  "Closed": "終了",
  "Closing soon": "まもなく終了",
  "Close pane": "ペインを閉じる",
  Close: "閉じる",
  "Collapse post": "投稿を折りたたむ",
  "Confirm boost": "ブーストを確認",
  "Confirm favorite": "お気に入りを確認",
  "Confirm follow": "フォローを確認",
  "Confirm unfollow": "フォロー解除を確認",
  "Connecting to {domain}...": "{domain} に接続中...",
  "Connecting to Bluesky...": "Bluesky に接続中...",
  "Content warning": "コンテンツ警告",
  "CW behavior": "CW の動作",
  Copy: "コピー",
  Copied: "コピーしました",
  "Copy diagnostics": "診断情報をコピー",
  "Copy text": "本文をコピー",
  "Copy URL": "URL をコピー",
  "{count} voters": "{count} 人",
  "{count}m left": "残り {count} 分",
  "{count}h left": "残り {count} 時間",
  "{count}d left": "残り {count} 日",
  Component: "構成要素",
  Custom: "カスタム",
  "Current: {value}": "現在: {value}",
  Database: "データベース",
  "The database is busy. Please try again.":
    "データベースが処理中です。しばらくしてから再試行してください。",
  Debug: "デバッグ",
  Delete: "削除",
  "Delete post": "投稿を削除",
  "Delete this post? This cannot be undone.":
    "この投稿を削除しますか？この操作は元に戻せません。",
  "Desktop notifications disabled": "デスクトップ通知は無効です",
  "Desktop notifications enabled": "デスクトップ通知は有効です",
  "Desktop notifications from these users are muted.":
    "これらのユーザーからのデスクトップ通知はミュートされています。",
  "Disable desktop notifications": "デスクトップ通知を無効化",
  "Display mode": "表示モード",
  "Display filter": "表示フィルタ",
  Direct: "ダイレクト",
  Download: "ダウンロード",
  Edit: "編集",
  "Edit post": "投稿を編集",
  "Edit post ({shortcut})": "投稿を編集 ({shortcut})",
  Emoji: "絵文字",
  "Enable desktop notifications": "デスクトップ通知を有効化",
  "Empty Pane": "空のペイン",
  "Exclude boosts": "boostを含まない",
  "Exclude media": "メディアを含まない",
  "Enter your instance domain to log in":
    "ログインするインスタンスのドメインを入力してください",
  "Expand post": "投稿を展開",
  "Favorite": "お気に入り",
  "Favorites": "お気に入り",
  "Favorite this post by {subject}?": "{subject} の投稿をお気に入りにしますか？",
  "Fetch": "取得",
  "Fetch lists": "リストを取得",
  "Fetching lists": "リストを取得中",
  "Find AIR context": "空中コンテキストを検索",
  "File logging": "ファイルログ",
  "Generate diagnostics": "診断情報を生成",
  "The operation failed. Please try again.":
    "操作に失敗しました。もう一度お試しください。",
  "The operation timed out. Please try again.":
    "操作がタイムアウトしました。もう一度お試しください。",
  Flags: "旗",
  "Follow": "フォロー",
  "Follow {subject}?": "{subject} をフォローしますか？",
  "Followers": "フォロワー",
  "Following": "フォロー中",
  "Follows you": "被フォロー",
  Food: "食べ物",
  "Food & Drink": "食べ物と飲み物",
  "Font size": "フォントサイズ",
  "Hashtag suggestions": "ハッシュタグ候補",
  Green: "グリーン",
  "Home": "ホーム",
  Hide: "隠す",
  "Hide media": "メディアを隠す",
  "Include media": "メディアを含む",
  "Instance domain": "インスタンスのドメイン",
  "In-memory support bundle": "メモリ上のサポートバンドル",
  "The request is invalid. Please review the input.":
    "入力内容が正しくありません。内容を確認してください。",
  "Instance login": "インスタンスへログイン",
  Error: "エラー",
  Warn: "警告",
  Info: "情報",
  Trace: "トレース",
  "Last 24h": "直近 24 時間",
  Jumbomoji: "ジャンボ絵文字",
  Keyword: "キーワード",
  Lavender: "ラベンダー",
  "List": "リスト",
  "Load More": "さらに読み込む",
  "Loading": "読み込み中",
  "Loading Awayuki": "Awayuki を読み込み中",
  "Loading initial timelines": "最初のタイムラインを読み込み中",
  "Loading media": "メディアを読み込み中",
  "Loading...": "読み込み中...",
  "Local": "ローカル",
  "Log in": "ログイン",
  "Log level": "ログレベル",
  "Login failed.": "ログインに失敗しました。",
  "Logout": "ログアウト",
  "Max Statuses": "最大投稿数",
  Media: "メディア",
  "Media failed to load": "メディアの読み込みに失敗しました",
  "Media source": "メディアソース",
  "Media unavailable": "メディアを利用できません",
  Mention: "メンション",
  "Mention suggestions": "メンション候補",
  Menu: "メニュー",
  More: "その他",
  Mauve: "モーヴ",
  Medium: "中",
  Mystique: "Mystique",
  Multiple: "複数選択",
  Muted: "ミュート中",
  Mute: "ミュート",
  "Mute {acct}": "{acct} をミュート",
  Name: "名前",
  "Next emoji categories": "次の絵文字カテゴリ",
  "No lists": "リストがありません",
  "No more statuses.": "これ以上の投稿はありません。",
  "No muted users.": "ミュート中のユーザーはいません。",
  "No deadline": "期限なし",
  "No statuses loaded.": "読み込まれた投稿はありません。",
  "No timeline tabs.": "タイムラインタブがありません。",
  Notification: "通知",
  "Notify off": "通知オフ",
  "Notify on": "通知オン",
  "NSFW behavior": "NSFW の動作",
  Objects: "物",
  "Open in browser": "ブラウザで開く",
  "Open Log": "ログを開く",
  "The backend response was lost. Refresh before retrying a change.":
    "バックエンドからの応答を確認できませんでした。変更を再試行する前に更新してください。",
  "Open media preview": "メディアプレビューを開く",
  "Opening and checking the database": "データベースを開いて確認中",
  "Open profile": "プロフィールを開く",
  "Open quoted post": "引用投稿を開く",
  "Open thread": "スレッドを開く",
  "Option {index}": "選択肢 {index}",
  "or": "または",
  Pane: "ペイン",
  "Pane {index} ({count})": "ペイン {index} ({count})",
  "Parameter": "パラメーター",
  People: "人物",
  "People & Body": "人物と身体",
  Peach: "ピーチ",
  Performance: "パフォーマンス",
  Poll: "投票",
  "Pinned posts": "固定された投稿",
  "Post": "投稿",
  "Post ({shortcut})": "投稿 ({shortcut})",
  "Post text is empty": "投稿本文が空です",
  Posts: "投稿",
  "Preset visibility": "自動 visibility",
  "Previous emoji categories": "前の絵文字カテゴリ",
  Private: "フォロワー限定",
  Profile: "プロフィール",
  Public: "公開",
  Quote: "引用",
  "Ready": "準備完了",
  Reload: "再読み込み",
  "Refresh": "更新",
  Remote: "リモート",
  "Remove media": "メディアを削除",
  "Remove option": "選択肢を削除",
  "Remove Pane": "ペインを削除",
  "Remove preset": "プリセットを削除",
  "Reorder media": "メディアを並べ替え",
  Reply: "返信",
  "Requested": "リクエスト済み",
  "Reset zoom": "ズームをリセット",
  Retry: "再試行",
  "Retrying media": "メディアを再試行中",
  "Restoring database, settings, and accounts":
    "データベース、設定、アカウントを復元中",
  "Restoring account sessions": "アカウントセッションを復元中",
  "Reload the application to recover. You can copy diagnostics before reloading.":
    "復旧するにはアプリケーションを再読み込みしてください。再読み込みの前に診断情報をコピーできます。",
  "Reveal media": "メディアを表示",
  Save: "保存",
  Sapphire: "サファイア",
  "Schema Reference": "スキーマリファレンス",
  "Scroll to top": "先頭へスクロール",
  Search: "検索",
  "Search this user's bookmarks": "このユーザーのブックマークを検索",
  "Search... (?query for YQ)": "検索... (?query で YQ)",
  "Search emoji...": "絵文字を検索...",
  "Search Query": "検索クエリ",
  "Search results": "検索結果",
  "Select list": "リストを選択",
  Settings: "設定",
  "Single": "単一選択",
  "Show results": "結果を表示",
  "Show post application": "投稿アプリを表示",
  "Post application position": "投稿アプリの表示位置",
  "Above actions": "アクションの直上",
  "Next to timestamp": "投稿日時の隣",
  "Due to Fediverse limitations, remote instances or servers may not provide post application data.":
    "Fediverse の制限により、リモートインスタンス/サーバーでは投稿アプリ情報が提供されず表示できない場合があります。",
  Sidecar: "サイドカー",
  "Add Sidecar": "サイドカーを追加",
  "Remove Sidecar": "サイドカーを削除",
  "Main View": "メインView",
  "Left side": "左側",
  "Right side": "右側",
  "Return to sidecar URL": "設定したURLに戻る",
  "Reload sidecar": "サイドカーを再読み込み",
  UserStyle: "UserStyle",
  "Enable UserStyle": "UserStyleを有効化",
  "Applying UserStyle requires JavaScript injection into the Sidecar WebView and can affect the displayed site. Use it only with sites you trust.":
    "UserStyleの適用にはSidecar WebViewへのJavaScript注入が必要であり、表示先サイト上で任意のCSSを実行するリスクがあります。信頼できるサイトにのみ使用してください。",
  "Size": "サイズ",
  Circle: "円形",
  Large: "大",
  Red: "レッド",
  Rounded: "角丸",
  Server: "サーバー",
  "The server rate limit was reached. Please try again later.":
    "サーバーのレート制限に達しました。しばらくしてから再試行してください。",
  Small: "小",
  Square: "四角",
  "SQLite statuses": "SQLite 投稿数",
  StarryEyes: "StarryEyes",
  "Statuses": "投稿数",
  "Statuses created in the last 15 minutes": "直近 15 分に作成された投稿数",
  "Switch account": "アカウントを切り替え",
  Symbols: "記号",
  "This is you": "自分です",
  "This operation is not supported by the account.":
    "このアカウントでは操作がサポートされていません。",
  "this user": "このユーザー",
  Thread: "スレッド",
  "Thread target is invalid": "スレッド対象が不正です",
  "Thread target is missing": "スレッド対象がありません",
  "Timeline": "タイムライン",
  "Timeline renderer": "タイムラインレンダラー",
  "Timeline settings": "タイムライン設定",
  "Timeline source color": "タイムラインのソース色",
  "Translate posts": "翻訳機能",
  "Auto translate posts": "自動翻訳",
  "Translation engine": "翻訳エンジン",
  "Apple Intelligence Foundation Model": "Apple Intelligence Foundation Model",
  "Apple Translation Framework": "Apple Translation Framework",
  "Show translation": "翻訳を表示",
  "Show original": "原文を表示",
  "Translated from {language}": "{language}からの翻訳",
  "Translating...": "翻訳中...",
  "Translation failed": "翻訳に失敗しました",
  "Translation is only supported on macOS.":
    "翻訳機能は macOS でのみ利用できます。",
  "Translation is not supported on this OS.":
    "この OS では翻訳機能を利用できません。",
  "Unknown language": "不明な言語",
  Transparent: "透明",
  "Travel & Places": "旅行と場所",
  Type: "種類",
  "Type: {type}": "種類: {type}",
  Unblock: "ブロック解除",
  "Unblock {acct}": "{acct} のブロックを解除",
  Unfollow: "フォロー解除",
  "Unfollow {subject}?": "{subject} のフォローを解除しますか？",
  Unlisted: "未収載",
  Unmute: "ミュート解除",
  "Unmute {acct}": "{acct} のミュートを解除",
  "Updated {duration} ago": "{duration} 前に更新",
  "Username or email": "ユーザー名またはメールアドレス",
  "Vacuum": "Vacuum",
  VirtualList: "仮想リスト",
  Vote: "投票",
  Visibility: "visibility",
  Width: "幅",
  "Move preset down": "プリセットを下へ移動",
  "Move preset up": "プリセットを上へ移動",
  Yellow: "イエロー",
  "YQ Query": "YQ クエリ",
  "YQ Reference": "YQ リファレンス",
  "Yukari Query Wiki": "Yukari Query Wiki",
  "What's on your mind?": "いまどうしてる？",
  "remaining": "残り",
  "used": "使用済み",
  "Policy": "ポリシー",
  "Preparing Awayuki": "Awayuki を準備中",
  "{remaining} / {limit} remaining ({used} used)":
    "残り {remaining} / {limit}（使用済み {used}）",
  "Resets in {reset} · Updated {updated} ago{policy}":
    "{reset} 後にリセット · {updated} 前に更新{policy}",
  " · Policy: {policy}": " · ポリシー: {policy}",
} as const;

export type MessageId = keyof typeof jaMessages;

const semanticEnglishMessages: Partial<Record<MessageId, string>> = {
  "timeline.home": "Home",
  "timeline.public": "Federated",
  "timeline.local": "Local",
  "timeline.notification": "Notification",
  "timeline.bookmarks": "Bookmarks",
  "timeline.favourites": "Favorites",
  "timeline.hashtag": "Hashtag",
  "timeline.list": "List",
  "timeline.custom": "Custom",
  "timeline.yq": "YQ",
  "timeline.search": "Search",
  "timeline.userBookmarks": "Bookmarks",
  "timeline.thread": "Thread",
  "timeline.profile": "Profile",
  "timeline.airContext": "AIR context",
  "timeline.unknown": "Unknown timeline ({type})",
  "timeline.unsupported":
    "Timeline type ({type}) is not supported by this version",
  "timeline.empty": "No statuses loaded.",
  "a11y.menu": "Menu",
  "a11y.dialog.close": "Close dialog",
  "a11y.media.moved": "Media moved to position {position}",
  "settings.section.account": "Account",
  "settings.section.appearance": "Appearance",
  "settings.section.behavior": "Behavior",
  "settings.section.performance": "Performance",
  "settings.section.notification": "Notification",
  "settings.section.timeline": "Timeline",
  "settings.section.sidecar": "Sidecar",
  "settings.section.database": "Database",
  "settings.section.debug": "Debug",
  "settings.section.about": "About",
};

/** Explicit EN dictionary; legacy IDs intentionally retain their English ID. */
export const enMessages = Object.freeze(
  Object.fromEntries(
    (Object.keys(jaMessages) as MessageId[]).map((id) => [
      id,
      semanticEnglishMessages[id] ?? id,
    ]),
  ) as Record<MessageId, string>,
);

export const ja = jaMessages satisfies Record<MessageId, string>;

export function messageCatalog(locale: AppLocale): Readonly<Record<MessageId, string>> {
  return locale === "ja" ? jaMessages : enMessages;
}

export function hasMessageId(value: string): value is MessageId {
  return Object.prototype.hasOwnProperty.call(jaMessages, value);
}

/** For controlled external catalogs (emoji groups, schema labels), not UI literals. */
export function translateKnownMessage(value: string): string {
  return hasMessageId(value) ? t(value) : value;
}

export function t(key: MessageId, values?: TranslationValues) {
  const template = messageCatalog(appLocale)[key];
  if (!values) return template;
  return template.replace(/\{(\w+)\}/g, (_, name: string) =>
    String(values[name] ?? `{${name}}`),
  );
}

export function intlFormatter<
  T extends Intl.DateTimeFormat | Intl.NumberFormat | Intl.RelativeTimeFormat,
>(
  factory: (locale: AppLocale) => T,
): T {
  return factory(appLocale);
}
