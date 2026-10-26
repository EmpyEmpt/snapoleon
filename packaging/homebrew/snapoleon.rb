cask "snapoleon" do
  version "${{ VERSION }}"
  sha256 "${{ SHA256_MACOS_UNIVERSAL_DMG }}"

  url "https://github.com/EmpyEmpt/snapoleon/releases/download/v#{version}/Snapoleon-#{version}-macos-universal.dmg"
  name "Snapoleon"
  desc "Screenshots, screen recording, OCR and sharing"
  homepage "https://github.com/EmpyEmpt/snapoleon"

  livecheck do
    url :url
    strategy :github_latest
  end

  auto_updates false
  depends_on macos: ">= :sonoma"

  app "Snapoleon.app"
  # A shim script, not a symlink to the executable: macOS knows an app by the path it was started from, and a
  # symlinked executable runs without its bundle (no bundle id, no Info.plist, privacy grants that never apply).
  shimscript = "#{staged_path}/snapoleon.wrapper.sh"
  binary shimscript, target: "snapoleon"

  preflight do
    File.write shimscript, <<~EOS
      #!/bin/sh
      exec '#{appdir}/Snapoleon.app/Contents/MacOS/snapoleon' "$@"
    EOS
  end

  uninstall quit:      "app.snapoleon",
            launchctl: "app.snapoleon"

  zap trash: [
    "~/Library/Application Support/app.Snapoleon.Snapoleon",
    "~/Library/Caches/app.Snapoleon.Snapoleon",
    "~/Library/LaunchAgents/app.snapoleon.plist",
    "~/Library/Preferences/app.snapoleon.plist",
  ]
end
