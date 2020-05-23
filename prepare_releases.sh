#!/bin/bash
set -e

./build.sh

# Update the database
./lcsc-scrape manage clear-database
./run.sh

./create-release.sh