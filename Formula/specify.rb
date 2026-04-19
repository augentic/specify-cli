# Homebrew formula for Specify.
#
# The SHA256 values below are placeholders. They MUST be recomputed for each
# release from the artifacts uploaded by .github/workflows/release.yml.
#
# Recommended updater: `brew bump-formula-pr`, which rewrites `url`, `version`,
# and `sha256` in one PR. Example after cutting v0.2.0:
#
#   brew bump-formula-pr \
#     --url=https://github.com/augentic/specify/releases/download/v0.2.0/specify-v0.2.0-aarch64-apple-darwin.tar.gz \
#     augentic/tap/specify
#
# See docs/release.md for the full post-tag update procedure.
class Specify < Formula
  desc "Deterministic operations for spec-driven development"
  homepage "https://github.com/augentic/specify"
  version "0.1.0"
  license "MIT OR Apache-2.0"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/augentic/specify/releases/download/v#{version}/specify-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_MACOS_ARM64_SHA256"
    else
      url "https://github.com/augentic/specify/releases/download/v#{version}/specify-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_MACOS_X86_64_SHA256"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/augentic/specify/releases/download/v#{version}/specify-v#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "PLACEHOLDER_LINUX_ARM64_SHA256"
    else
      url "https://github.com/augentic/specify/releases/download/v#{version}/specify-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "PLACEHOLDER_LINUX_X86_64_SHA256"
    end
  end

  def install
    bin.install "specify"
  end

  test do
    assert_match "specify #{version}", shell_output("#{bin}/specify --version")
  end
end
