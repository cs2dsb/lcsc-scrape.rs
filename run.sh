#!/bin/bash

set -e

./build.sh

# This downloads data for LCSC and JLC into the cache but doesn't process it, useful if you want to
# kick off the download on an always on server before copying the cache dir locally
./lcsc-scrape fetch --jlc-data --download-to-cache-only --parts-spreadsheet JLC_Components.ods

# This parses the data and creates/updates the sqlite DB (it will also download if the data isn't in the cache)
./lcsc-scrape fetch --jlc-data --parts-spreadsheet JLC_Components.ods

# This parses various floats, bools and prices into new pp_ columns of the correct type
./lcsc-scrape manage post-process