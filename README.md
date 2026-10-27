# LisekMovie-plugin-omdb

OMDb metadata provider extension for [LisekMovie](https://github.com/LisekMovieApp/LisekMovie-app), the self-hosted video-on-demand server.

| | |
|---|---|
| Provider key | `omdb` |
| Covers | Movies, series |
| Status | Basic — works, parts still missing |
| API key | Required — free key (1,000 requests/day) from https://www.omdbapi.com/apikey.aspx |
| Fix match ids | `tt0133093` and IMDb links (ids are IMDb ids) |

## What it does

OMDb API (IMDb data): search by title and year, lookup by IMDb id with plot, genres, runtime, IMDb rating, US certification (→ age rating) and poster.

## How it plugs in

The crate implements `lisek_plugin_api::MetadataProvider` and is compiled into `lisek-server` as a Cargo path dependency — there is no runtime plugin loading. Clone it **next to** `LisekMovie-app`:

```
LisekMovie/
  LisekMovie-app/
  LisekMovie-plugin-omdb/
  ...
```

The server lists the provider under **Admin → Plugins** (disabled by default). Turn it on, add the key if it needs one, and set its priority — globally, or per library (for example AniList or aniDB first in an anime library). `install-linux.sh` and `update-linux.sh` clone and update it with the other modules.

## Development

```bash
cargo clippy --all-targets -- -D warnings
cargo test
```

Tests run against a local `wiremock` server; nothing calls the real service.

## License

[MIT](LICENSE), like LisekMovie itself.
