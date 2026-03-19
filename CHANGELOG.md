# Changelog

All notable changes to toddy will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [Unreleased]

## [0.3.1] - 2026-03-19

### Fixed

- Preserve iced widget defaults when props are unset. Padding,
  spacing, text size, and other optional props now use `Option`
  return types from parsers. When absent from the wire message,
  the widget setter is skipped and iced uses its built-in default.
  Affected widgets: button, container, window, column, row, grid,
  keyed_column, text_input, pick_list, combo_box, table.

## [0.3.0] - 2026-03-19

Initial public release.
