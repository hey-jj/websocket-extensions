# Changelog

## [0.2.0] - 2026-07-07

### Changed
- Empty client offers no longer make RSV bits valid before activation. (#16)
- Client activation now rejects duplicate accepted responses for the same extension. (#17)
- Quoted parameter values now keep escaped bytes, reject escaped control characters, and serialize backslashes with escaping. (#19)
- Repeated server response generation now recomputes RSV reservations for the current offer. (#20)

### Fixed
- Header parsing now accepts tab whitespace after commas. (#18)

## [0.2.0] - 2026-07-07

### Changed
- Empty client offers no longer make RSV bits valid before activation. (#16)
- Client activation now rejects duplicate accepted responses for the same extension. (#17)
- Quoted parameter values now keep escaped bytes, reject escaped control characters, and serialize backslashes with escaping. (#19)
- Repeated server response generation now recomputes RSV reservations for the current offer. (#20)

### Fixed
- Header parsing now accepts tab whitespace after commas. (#18)
