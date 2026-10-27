# Finder layout for the Recursa disk image; tools/dmg.sh runs dmgbuild here with the app in `app`.
# Paths are relative to this directory. background.tiff is background.svg rendered at 1x and 2x by
# `swift render.swift`; rerun it after editing the SVG.
files = [defines["app"]]
symlinks = {"Applications": "/Applications"}
hide_extensions = ["Recursa.app"]
hide = [".VolumeIcon.icns", ".background.tiff"]
icon = "../icons/icon.icns"
background = "background.tiff"
format = "UDZO"
window_rect = ((200, 120), (660, 400))
default_view = "icon-view"
icon_size = 128
text_size = 13
icon_locations = {"Recursa.app": (165, 180), "Applications": (495, 180)}
