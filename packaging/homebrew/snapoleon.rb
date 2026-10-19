cask "snapoleon" do
  version "${{ VERSION }}"
  sha256 "${{ SHA256_MACOS_UNIVERSAL_DMG }}"

  url "https://github.com/snapoleon/snapoleon/releases/download/v#{version}/Snapoleon-#{version}-macos-universal.dmg"
  name "Snapoleon"
  desc "Screenshots, screen recording, OCR and sharing"
  homepage "https://github.com/snapoleon/snapoleon"

  livecheck do
    url :url
    strategy :github_latest
  end

  auto_updates false
  depends_on macos: ">= :monterey"

  app "Snapoleon.app"
  binary "#{appdir}/Snapoleon.app/Contents/MacOS/snapoleon"

  zap trash: [
    "~/Library/Application Support/app.Snapoleon.Snapoleon",
    "~/Library/Caches/app.Snapoleon.Snapoleon",
    "~/Library/Preferences/app.snapoleon.plist",
  ]
end
