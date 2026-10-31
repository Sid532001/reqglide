# Reqglide product website

The landing page and guide use static HTML, CSS, and browser JavaScript. There
are no external fonts, trackers, forms, runtime services, or framework builds.

From the repository root:

```shell
pnpm site:dev
pnpm site:build
```

Development serves `http://127.0.0.1:4173`. The static output is `apps/site/dist`;
its `index.html` also works through `file://`. Rebuild after editing source.

`site.config.json` defines the repository, canonical website URL, and preview
version. Public metadata for the installers lives in `releases/<version>.json`.
The build checks version consistency and validates metadata. When local Windows
artifacts exist, it verifies their sizes and SHA-256 hashes before including
them in the generated site's `downloads` directory. Generated files and
installer binaries do not belong in Git history.

For publication:

```shell
node scripts/site.mjs build --remote-downloads
```

This links to the versioned GitHub release and keeps the website artifact small.
The `Product website` workflow builds and publishes to GitHub Pages on changes
to website source. Repository Pages settings must use GitHub Actions.

Prepare a community preview from clean, committed source with
`scripts/build-windows.ps1 -CommunityPreview`, then run
`node scripts/site.mjs capture-release` and commit the new checksum metadata.
Upload the actual installers and checksum list to the matching GitHub
prerelease before publishing the updated page. Keep unsigned preview notices
until a signed production release is verified.

Before shipping, check desktop and mobile layouts, keyboard navigation,
example tabs/copy, guide links, installer downloads, checksums, and direct-file
opening. The product screenshot is a real app capture using synthetic example
data, rather than a customer workspace.
