# Social-card font inputs

These are the unchanged TTF inputs used by the site's existing social cards.
They are kept in source control so `generate-og.mjs` does not depend on its
previous ignored `.fonts` cache or fetch mutable `@latest` / `master` URLs.
The browser fonts still come from the locked Fontsource npm packages.

| File | Embedded version field | SHA-256 |
| --- | --- | --- |
| `Inter-Bold.ttf` | `Version 4.001;git-66647c0bb` | `f4d309afb4777beb63e18eb322f7d721bc6dcd7520922019d2cb14dddf279bb3` |
| `Inter-Regular.ttf` | `Version 4.001;git-66647c0bb` | `d3641a4ba1f6c109b1ef87b3cd32262b4a10dec9e61274dd749bdd3f75437d91` |
| `JetBrainsMono-Bold.ttf` | `Version 2.211` | `ff862f6a26a80ee216a96f5d7100c8389513cac26b53b7a665d9ace29b970b04` |

Provenance: retained from the site's existing `scripts/.fonts` inputs on
2 October 2026. The earlier downloader used Fontsource CDN URLs, with
Inter and JetBrains Mono upstream repositories as fallbacks. It did not
record the download source or release version. The version fields above were
read directly from these TTFs; upstream release identity is not independently
verified for these retained bytes.

Inter and JetBrains Mono are distributed under the SIL Open Font License 1.1.
The license notices shipped with the site are
[`Inter-OFL-1.1.txt`](../../public/licenses/Inter-OFL-1.1.txt) and
[`JetBrainsMono-OFL-1.1.txt`](../../public/licenses/JetBrainsMono-OFL-1.1.txt).
