#!/bin/bash

set -e

./build.sh

# This parses the data and creates/updates the sqlite DB (it will also download if the data isn't in the cache)
./lcsc-scrape fetch --jlc-data --parts-spreadsheet JLC_Components.ods

# This parses various floats, bools and prices into new pp_ columns of the correct type
./lcsc-scrape manage post-process
