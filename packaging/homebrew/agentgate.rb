# Legacy compatibility only. New users should install the muxlayer cask.
cask "agentgate" do
  arch arm: "aarch64", intel: "x64"

  version "2.0.7"
  sha256 arm:   "7bdd08ee99a29470f2ba05a1d3ded2a187f3d578419fccd75c715ee8160812bc",
         intel: "07fb46c6f53c32b68c3083b4540733cefbc50c2464a79beb78bed9124250292e"

  url "https://github.com/dengmengmian/muxlayer/releases/download/v#{version}/MuxLayer_#{version}_#{arch}.dmg",
      verified: "github.com/dengmengmian/muxlayer/"
  name "MuxLayer (formerly AgentGate)"
  desc "Legacy cask for the MuxLayer local model control layer"
  homepage "https://dengmengmian.github.io/muxlayer/"

  livecheck do
    url :url
    strategy :github_latest
  end

  auto_updates true
  conflicts_with cask: "muxlayer"
  depends_on :macos

  app "MuxLayer.app"

  zap trash: [
    "~/Library/Application Support/com.mengmian.agentgate",
    "~/Library/Caches/com.mengmian.agentgate",
    "~/Library/Preferences/com.mengmian.agentgate.plist",
    "~/Library/Saved Application State/com.mengmian.agentgate.savedState",
    "~/Library/WebKit/com.mengmian.agentgate",
  ]

  caveats <<~EOS
    This cask remains available so existing AgentGate installations keep upgrading.
    New installations should use:
      brew install --cask dengmengmian/tap/muxlayer

    To adopt the new cask name without deleting settings:
      brew uninstall --cask agentgate
      brew install --cask dengmengmian/tap/muxlayer
  EOS
end
