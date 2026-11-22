#!/bin/sh
# Print the Homebrew formula for a fire release.
# Usage: homebrew-formula.sh <version without v> <checksums.txt>
set -eu
VERSION="$1"
SUMS="$2"

sha() {
  file="fire-v${VERSION}-$1.tar.gz"
  value="$(awk -v f="$file" '$2 == f { print $1 }' "$SUMS")"
  [ -n "$value" ] || { echo "missing checksum for $file" >&2; exit 1; }
  echo "$value"
}

MAC_ARM="$(sha macos-arm64)"
MAC_INTEL="$(sha macos-intel)"
LINUX_ARM="$(sha linux-arm64)"
LINUX_INTEL="$(sha linux-x86_64)"

cat <<RUBY
class Fire < Formula
  desc "Config-first task runner with completion, detachable sessions, and an MCP server"
  homepage "https://github.com/gbenm/fire"
  version "${VERSION}"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/gbenm/fire/releases/download/v#{version}/fire-v#{version}-macos-arm64.tar.gz"
      sha256 "${MAC_ARM}"
    end

    on_intel do
      url "https://github.com/gbenm/fire/releases/download/v#{version}/fire-v#{version}-macos-intel.tar.gz"
      sha256 "${MAC_INTEL}"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/gbenm/fire/releases/download/v#{version}/fire-v#{version}-linux-arm64.tar.gz"
      sha256 "${LINUX_ARM}"
    end

    on_intel do
      url "https://github.com/gbenm/fire/releases/download/v#{version}/fire-v#{version}-linux-x86_64.tar.gz"
      sha256 "${LINUX_INTEL}"
    end
  end

  def install
    bin.install "fire"
  end

  test do
    assert_match "Fire CLI", shell_output("#{bin}/fire")
  end
end
RUBY
