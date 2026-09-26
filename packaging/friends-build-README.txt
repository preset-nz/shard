{{PRODUCT_NAME}}: free, pre-release build. Apple Silicon (M-series) Macs only.

It isn't signed, so the first time you open it macOS says "{{PRODUCT_NAME}}.app is damaged and can't be opened". It isn't damaged; macOS doesn't recognise the developer. Fix it once: open Terminal (Cmd+Space, type Terminal), type xattr -cr followed by a space, drag {{PRODUCT_NAME}}.app into the Terminal window and press Return. Double-click the app again and it opens from then on.

Found a bug? Email georg@preset.nz. A screenshot helps.
