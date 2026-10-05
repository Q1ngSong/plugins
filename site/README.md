# 项目主页

主页地址是 <https://q1ngsong.github.io/plugins/>。页面从上到下是：首屏、两张下载卡片、根目录 README 的全文。

## 文件

| 文件 | 内容 |
|---|---|
| `site/index.html` | 模板：首屏和下载卡片写在这里，`__README__` 的位置填 README |
| `site/style.css` | 样式，配色和 `assets/` 里的插图一致，跟着系统深浅色变 |
| `site/build.py` | 把 README 填进模板，写出根目录的 `index.html` |
| 根目录的 `index.html` | 生成出来的页面，GitHub Pages 发布的就是它 |

README 开头的标题、介绍和主图已经在首屏里，所以页面上的 README 从第一个二级标题（「起因」）开始。README 里的相对链接会改成指向 GitHub 上的文件，插图和样式直接用仓库里的 `assets/` 和 `site/style.css`。

## 生成和预览

```sh
python3 -m pip install markdown-it-py
python3 site/build.py
python3 -m http.server 8770
```

打开 <http://localhost:8770/>。

改了 README 或模板之后要重新生成，把根目录的 `index.html` 一起提交。只改源文件，主页不会变。

## 发布

在 GitHub 仓库的 Settings → Pages 里，Source 选 **Deploy from a branch**，分支选 **main**，目录选 **/ (root)**。之后每次推送 main，主页会跟着更新。根目录的 `.nojekyll` 让 Pages 原样发布文件，不经过 Jekyll。
