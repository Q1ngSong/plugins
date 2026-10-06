/* 插件中心自己有没有新版本：问 GitHub 上最新一次正式发布（草稿和预发布不算），和正在运行的版本比。
   直接在页面里问，窗口版和浏览器版都走系统代理；后端不用动 */

/** GitHub 上这个项目最新一次正式发布的接口 */
const LATEST_RELEASE = "https://api.github.com/repos/Q1ngSong/plugins/releases/latest";

export interface Release {
  /** 版本号，不带开头的 v */
  version: string;
  /** 发布页，安装包在这里下载 */
  url: string;
  /** 发布说明（Markdown 原文） */
  notes: string;
  /** 发布时间 */
  at: string;
}

/** 最新一次正式发布；一个版本都没发过时是 null。网络不通时抛错，由调用方决定要不要提示 */
export async function latestRelease(): Promise<Release | null> {
  const r = await fetch(LATEST_RELEASE);
  if (r.status === 404) return null;
  if (!r.ok) throw new Error(`GitHub 回了 ${r.status}`);
  const d = await r.json();
  return { version: String(d.tag_name ?? "").replace(/^v/, ""), url: String(d.html_url ?? ""), notes: String(d.body ?? ""), at: String(d.published_at ?? "") };
}

/** a 是不是比 b 新：按「主.次.修」三段数字比；不是这个格式的（比如带 beta）就当不新 */
export function isNewer(a: string, b: string): boolean {
  const pa = numbers(a), pb = numbers(b);
  if (!pa || !pb) return false;
  for (let i = 0; i < 3; i++) if (pa[i] !== pb[i]) return pa[i] > pb[i];
  return false;
}

const numbers = (v: string) => {
  const m = /^(\d+)\.(\d+)\.(\d+)$/.exec(v.trim());
  return m && [+m[1], +m[2], +m[3]];
};
