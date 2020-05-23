#!/bin/bash
set -e

timestamp=`date "+%Y-%m-%d_%H%M%S"`
mkdir -p releases


# Create the executable and db release
tar -zchf releases/${timestamp}_executable_and_db_linux_amd64.tar.gz ./lcsc-scrape ./lcsc.sqlite

# Create the cache release
tar -zchf releases/${timestamp}_cache.tar.gz ./cache