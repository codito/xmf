# xmf

A simple mutual funds and stocks tracker application written in Rust.

## Features

- [x] Tracks multiple portfolios using a simple yaml based configuration
- [x] Multiple investment types
  - [x] Stocks
  - [x] Mutual funds
  - [x] Fixed deposits (manually updated)
- [x] Supports multiple data backends
  - [x] Use Yahoo Finance! API tickers
  - [x] Use ISIN tickers for Indian Mutual Funds

App is work-in-progress.

## Usage

1. Clone this repo.
2. Run `cargo run -- setup` to create a sample config file.
3. Add your investments in the config file.
4. Run `cargo run` to fetch a summary.

## Credits

Built with AI. Thanks to Yahoo Finance and [captnemo's MF
API](https://mf.captnemo.in).

## License

MIT
