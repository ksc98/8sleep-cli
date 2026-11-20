#!/usr/bin/env bash
# Write Formula/eightsleep.rb in the tap checkout for release vVERSION, from
# the prebuilt binaries the release workflow attached. Usage: scripts/formula.sh 0.1.3
set -euo pipefail

v=${1:?usage: scripts/formula.sh VERSION (e.g. 0.1.3)}
tap=${TAP:-$HOME/dev/homebrew-tap}
base="https://github.com/ksc98/8sleep-cli/releases/download/v$v"
targets=(aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu)

declare -A sha
assets=$(gh release view "v$v" -R ksc98/8sleep-cli --json assets --jq '.assets[] | "\(.name) \(.digest)"')
for t in "${targets[@]}"; do
    d=$(awk -v n="8sleep-$t.tar.gz" '$1 == n { sub("sha256:", "", $2); print $2 }' <<<"$assets")
    [[ $d =~ ^[0-9a-f]{64}$ ]] || { echo "release v$v has no 8sleep-$t.tar.gz yet" >&2; exit 1; }
    sha[$t]=$d
done

cat >"$tap/Formula/eightsleep.rb" <<EOF
class Eightsleep < Formula
  desc "Command-line control for the Eight Sleep Pod"
  homepage "https://github.com/ksc98/8sleep-cli"
  version "$v"
  license "MIT"

  on_macos do
    on_arm do
      url "$base/8sleep-aarch64-apple-darwin.tar.gz"
      sha256 "${sha[aarch64-apple-darwin]}"
    end
    on_intel do
      url "$base/8sleep-x86_64-apple-darwin.tar.gz"
      sha256 "${sha[x86_64-apple-darwin]}"
    end
  end

  on_linux do
    on_arm do
      url "$base/8sleep-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "${sha[aarch64-unknown-linux-gnu]}"
    end
    on_intel do
      url "$base/8sleep-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "${sha[x86_64-unknown-linux-gnu]}"
    end
  end

  def install
    bin.install "8sleep"
  end

  def caveats
    <<~EOS
      Sign in with your Eight Sleep email and password:
        8sleep setup
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/8sleep --version")
    assert_match "no Eight Sleep credentials",
      shell_output("XDG_CONFIG_HOME=#{testpath} XDG_CACHE_HOME=#{testpath} #{bin}/8sleep status 2>&1", 1)
  end
end
EOF
echo "wrote $tap/Formula/eightsleep.rb for v$v"
