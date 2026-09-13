# Homebrew Cask 发布

本目录维护两个 cask 的参考副本，通过自有 tap `dengmengmian/homebrew-tap` 分发（不需要进 homebrew-core）：

| 文件 | cask token | 用途 |
|---|---|---|
| `muxlayer.rb` | `muxlayer` | 主 cask，新用户安装这个 |
| `agentgate.rb` | `agentgate` | 旧名兼容 cask，保证已有 AgentGate 安装继续升级；与 `muxlayer` 互斥（`conflicts_with`） |

两个 cask 装的是同一个 `MuxLayer.app`（同一份 DMG），`zap` 路径都指向 bundle identifier `com.mengmian.agentgate` 的目录。

## 首次建 tap（一次性）

1. 在 GitHub 创建公开仓库 `dengmengmian/homebrew-tap`。
2. 把本目录的 `muxlayer.rb` 和 `agentgate.rb` 复制到该仓库的 `Casks/` 下并推送。
3. 用户即可安装：

```bash
brew install --cask dengmengmian/tap/muxlayer
```

安装命令已写进 README.md / README_ZH.md 的 Download 区块和官网下载区（`pnpm docs:download:check` 会校验主命令是 `muxlayer`）。

## 每次发版更新

release.yml 的 `homebrew-cask` job 在 `publish` job 把 release 转为正式版之后运行，自动更新 tap 仓库里的
`Casks/muxlayer.rb` 和 `Casks/agentgate.rb`（version + arm / intel 两个 DMG 的 sha256），两个文件缺一个都会让 job 失败。

需要仓库 Secret `HOMEBREW_TAP_SSH_KEY`：`dengmengmian/homebrew-tap` 上一把**带写权限的 deploy key** 的私钥。
用 deploy key 而不是 PAT，是因为 fine-grained PAT 强制带过期时间，到期后 job 会在 clone 处失效；deploy key 不过期，且只能操作这一个仓库。
push 走 SSH，workflow 的 `GITHUB_TOKEN` 只用来读 release 资产，不需要额外权限。

生成与配置 deploy key：

```bash
ssh-keygen -t ed25519 -C "muxlayer release -> homebrew-tap" -f tap_deploy_key -N ""
# tap_deploy_key.pub → dengmengmian/homebrew-tap → Settings → Deploy keys（勾选 Allow write access）
# tap_deploy_key     → dengmengmian/muxlayer → Settings → Secrets → Actions → HOMEBREW_TAP_SSH_KEY
```

本目录的两个 `.rb` 是模板/参考副本：`scripts/bump-version.sh` 只同步其中的 `version`，sha256 以 tap 仓库为准；结构改动时需与 tap 仓库同步。

## 本地验证

```bash
brew style --cask packaging/homebrew/muxlayer.rb packaging/homebrew/agentgate.rb
brew install --cask packaging/homebrew/muxlayer.rb
```
