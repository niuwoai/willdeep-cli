# Bundled willdeep-config

Source: https://github.com/niuwoai/willdeep-config

Pinned commit: `c17f639ef676ca9e8625033ebd5f44ef6ed5c8ee` (willdeep-config main), package version `0.6.0-rc2`.

The bundled package has since diverged from that commit. From `0.6.0-rc3` the
configuration page is edited in this repository: a global default model, token
presets, and the provider and subagent keys the parser already accepts. The
upstream repository has not been updated with those changes.

The `willdeep-config/` directory contains the plugin package. First-use Web setup installs this package only when no version of
`willdeep-config` is installed. It never replaces an existing package or grants
approval. Users review its declared permissions in the normal plugin center.

The server requires `/usr/bin/ruby`; hosts without it can use terminal onboarding.
The Web host sets `WD_CONFIG_FILE` and `WD_CONFIG_BACKUP_DIR` for this plugin so
editing and backups use the selected configuration and WillDeep home.
