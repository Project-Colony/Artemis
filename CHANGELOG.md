# Changelog

## [0.1.1](https://github.com/Project-Colony/Artemis/compare/v0.1.0...v0.1.1) (2026-10-14)


### Fixes

* **client:** cut reply previews on character boundaries ([#14](https://github.com/Project-Colony/Artemis/issues/14)) ([031597b](https://github.com/Project-Colony/Artemis/commit/031597b230053d8a8fc83459d1e9461def6efc99))
* make sign-in, encrypted direct messages, reconnection and server features work end to end ([#22](https://github.com/Project-Colony/Artemis/issues/22)) ([3b7b72d](https://github.com/Project-Colony/Artemis/commit/3b7b72dd33a88a2644e732e8b1beb5e5750c78cc))
* **relay:** bound unauthenticated sockets, frame sizes and event rates ([#20](https://github.com/Project-Colony/Artemis/issues/20)) ([e3f1721](https://github.com/Project-Colony/Artemis/commit/e3f1721a3ca4103e668e033ef5036d3441c9fd51))
* **relay:** check server membership on every channel request ([#17](https://github.com/Project-Colony/Artemis/issues/17)) ([2f073be](https://github.com/Project-Colony/Artemis/commit/2f073be8e4fb934791416eb9def5cd6d0c631804))
* **relay:** give each socket its own bounded send queue ([#19](https://github.com/Project-Colony/Artemis/issues/19)) ([5b33f7d](https://github.com/Project-Colony/Artemis/commit/5b33f7dcfa66b473edbce3bf767911474e23feda))
* **relay:** load .env and refuse missing or unsafe configuration ([#16](https://github.com/Project-Colony/Artemis/issues/16)) ([a38115b](https://github.com/Project-Colony/Artemis/commit/a38115b047fe5e36dad424846730deaab315662a))
* **relay:** record applied migrations so the relay restarts on an existing database ([#15](https://github.com/Project-Colony/Artemis/issues/15)) ([1d24541](https://github.com/Project-Colony/Artemis/commit/1d245414e95b339df1f02d2858526980c4fa9949))
* **relay:** send server events only to that server's members ([#18](https://github.com/Project-Colony/Artemis/issues/18)) ([ecd8ff2](https://github.com/Project-Colony/Artemis/commit/ecd8ff2a7bc2d6b609b70050ed5cd322084cba90))
* **relay:** validate input sizes and broadcast only what was stored ([#21](https://github.com/Project-Colony/Artemis/issues/21)) ([ed7d74e](https://github.com/Project-Colony/Artemis/commit/ed7d74e50e2b7329296db81c2e7285d93a15c3f2))
