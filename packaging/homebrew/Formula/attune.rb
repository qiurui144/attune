# Homebrew formula for Attune CLI + server.
#
# 发布到独立 tap 仓 qiurui144/homebrew-attune(详见 packaging/homebrew/SETUP.md).
# 不进 homebrew-core(项目暂不达 homebrew-core notability 门槛,且不需要 maintainer 审核延迟).
#
# 用户安装:
#   brew tap qiurui144/attune
#   brew install attune
#
# 用户升级:
#   brew upgrade attune
#
# 注:本 formula 装的是 server / CLI 二进制(`attune` 命令),不是 Tauri 桌面 GUI.
# 桌面应用 macOS 端走 GitHub release .dmg 或 cask(v1.1 规划).

class Attune < Formula
  desc "Private AI Knowledge Companion — local-first hybrid intelligence"
  homepage "https://engi-stack.com"
  version "1.0.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/qiurui144/attune/releases/download/v1.0.0/attune-macos-aarch64.tar.gz"
      # 首次发版后必须替换为实际 sha256(per packaging/homebrew/SETUP.md §Step 3)
      sha256 "REPLACE_WITH_ACTUAL_SHA256_FROM_RELEASE_ASSET"
    end
    # macOS Intel 暂无预编译产物,用户需自编(rust-release.yml 未覆盖 x86_64-apple-darwin)
  end

  on_linux do
    on_intel do
      url "https://github.com/qiurui144/attune/releases/download/v1.0.0/attune-linux-x86_64.tar.gz"
      sha256 "REPLACE_WITH_ACTUAL_SHA256_FROM_RELEASE_ASSET"
    end
    on_arm do
      url "https://github.com/qiurui144/attune/releases/download/v1.0.0/attune-linux-aarch64.tar.gz"
      sha256 "REPLACE_WITH_ACTUAL_SHA256_FROM_RELEASE_ASSET"
    end
  end

  def install
    bin.install "attune"
    # attune-server-headless 同 tarball 内,可选安装
    bin.install "attune-server-headless" if File.exist?("attune-server-headless")
  end

  def caveats
    <<~EOS
      Attune CLI installed.

      Quick start:
        attune --help                  # CLI usage
        attune-server-headless         # start headless server on :18900

      First-time setup walks through vault init + LLM provider config.
      See: https://github.com/qiurui144/attune#quick-start

      Desktop GUI (Tauri) is distributed via GitHub Releases on macOS:
        https://github.com/qiurui144/attune/releases/latest

      The CLI/server here is sufficient for headless use (Web UI on :18900).
    EOS
  end

  test do
    assert_match "attune", shell_output("#{bin}/attune --version")
  end
end
