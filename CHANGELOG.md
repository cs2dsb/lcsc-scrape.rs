# Change Log

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](http://keepachangelog.com/)
and this project adheres to [Semantic Versioning](http://semver.org/).

## [Unreleased]
### Added
- `--drop-price-columns` flag to query that removes all columns with "Price" in their name from query results
- `--omit-headers` flag to query to skip printing headers which makes it possible to pipe the output of a single column query into xargs | wget or whatever
- Rate limiting using `x-ratelimit-remaining` header and "TOO MANY REQUESTS" http error code
-- Inductance post-processing
### Changed
### Deprecated
### Removed
### Fixed
- Replaced print! and println! calls in query with write! and writeln! so broken pipes can be swallowed to avoid errors when piping to `head -n 1`
- Fixed for new JLCPCB parts api (it doesn't support url encoding now)
### Security
- Updated `calamine` to resolve https://github.com/advisories/GHSA-ppqp-78xx-3r38
