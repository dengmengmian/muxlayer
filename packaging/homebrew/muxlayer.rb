cask "muxlayer" do
  arch arm: "aarch64", intel: "x64"

  version "2.0.6"
  sha256 arm:   "4f71ad260ef17f2f8c3c35900c3540dac4052f81f5f3dc7b453f0a3f31a42e9f",
         intel: "b43afca9215baf5a84bb2c67df363b3f1fe236a5a2bfe39c302a55a0e40a1510"

  url "https://github.com/dengmengmian/muxlayer/releases/download/v#{version}/MuxLayer_#{version}_#{arch}.dmg",
      verified: "github.com/dengmengmian/muxlayer/"
  name "MuxLayer"
  desc "Local model control layer for coding agents"
  homepage "https://dengmengmian.github.io/muxlayer/"

  livecheck do
    url :url
    strategy :github_latest
  end

  auto_updates true
  conflicts_with cask: "agentgate"
  depends_on :macos

  app "MuxLayer.app"

  zap trash: [
    "~/Library/Application Support/com.mengmian.agentgate",
    "~/Library/Caches/com.mengmian.agentgate",
    "~/Library/Preferences/com.mengmian.agentgate.plist",
    "~/Library/Saved Application State/com.mengmian.agentgate.savedState",
    "~/Library/WebKit/com.mengmian.agentgate",
  ]
end
