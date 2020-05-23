# lcsc-scrape

A tool to scrape part info from LCSC  for parts available from the JLCPCB SMT assembly service into a local database for easier part lookup. You could use it to gather data about LCSC parts not in the JLCPCB SMT assembly service but that's not it's intended purpose.

## Using

If all you want to do is query the database you can just download the `..._executable_and_db.tar.gz` from releases. You can optionally download the `...cache.tar.gz` release to get the cache dir if you want to update the db. 

[Latest Release](https://github.com/cs2dsb/lcsc-scrape.rs/releases)

Currently releases are only built for linux-amd64 but if there's any demand for it I can build for more platforms - raise an issue. In the mean time you can build from source quite easily with cargo & rust.

**Strongly advise against running the scraper with an empty cache unless you have a good reason** see [Selectively refresh cache](#selectively-refresh-cache) for less drastic update options. Running a full update with no cache will a) take hours and b) hammer LCSC and JLC servers - I have no idea if they will have a problem with it but there's no reason to generate extra traffic when the cache is completely usable 99% of the time.

### Building & running from source

1. [Install rust & cargo](https://www.rust-lang.org/tools/install)
1. Clone the repo
1. `sudo ./bootstrap.sh` (to install the sqlite lib)
1. `./build.sh`
1. Download the latest `..._cache.tar.gz`
1. Extract the tar with `tar -xzf *_cache.tar.gz`
1. Make sure the `cache` directory from the tar is in the root of the repo
1. `./run.sh`. See [Updating local data](#updating-local-data) for more info/manual run commands
1. The progress stats that are printed as it runs should show "Downloaded: 0, Cached: #lots" after a few seconds. If the download number increases rapidly it's not using the cache. The program prints the canonical path to the cache directory as it's first output - check this matches where you put the cache.

## Description

This tool takes a list of LCSC part numbers (C#####) and fetches the LCSC product page and, optionally, the JLC SMT stock and prices.

The http response data is stored in a local cache to avoid hammering LCSC & JLCPCB - I've not had any issues but they could very well ban IPs for excessive requests, who knows.

Once the data has been stored in the cache, it is parsed to extract as much data as possible from it - both info tables and the prices on LCSC and the stock, basic/extended status and prices from JLCPCB. This data is collected in a SQLite database for querying.

There are also optional post-processing steps to convert the string data for certain columns (basic/extended, prices, voltages) into bool/float for better querying.

Repeated fetch and post-processing runs can be executed without having to re-download any data that's already in the cache. 

## Updating local data

`./run.sh` will do everything for you if you prefer (except updating the spreadsheet)

1. Get the latest parts spreadsheet from the JLC SMT part list page (button on the top-ish right)
2. Convert the spreadsheet to a valid ods or xlsx (There's something odd about the xls they offer for download that the spreadsheet library can't handle)
3. Build in release mode 
    ```
    cargo build --release
    ```
3. Run a fetch job (--jlc-data gets the prices, stock and basic/extended from the JLC SMT assembly service)
    ```
    ./lcsc-scrape fetch -v --jlc-data --parts-spreadsheet JLC_Components.ods
    ```
    If the cache is empty and you're doing the full 37k parts this will take several hours
4. Run a post process job to add float/bool versions of various well known columns
    ```
    ./lcsc-scrape manage post-process
    ```
5. Query the database
    ```
    ./lcsc-scrape query "SELECT * FROM parts"
    ```

## The Cache

The cache is just the html/json response body from the LCSC produc page and the JLC SMT search api.

The cache directory is included in the git repo so the majority of results will already be there if you pull the repo (it's ~3.5GB).

`./lcsc-scrape manage clear-cache` - this will delete the cache directory and make the next `fetch` get all new data. I don't recommend doing this since the majority of the data (esp LCSC) is static so it's just tempting fate that you'll be banned. But I'm not your mum, do it at your own risk if you like.

`./lcsc-scrape manage clear-database` - this will drop the database tables and vacuum it to reclaim free space. You could also just delete the .sqlite file. Useful if you change the post-processing code and want a clean dataset.

### Selectively refresh cache

There are several options for getting new data without completely fetching all new data:

`./lcsc-scrape fetch ... --parts C1234 C9292` - using `--parts`, `--parts-list` or `--parts-spreadsheet` with a smaller subset of parts (like just the ones you have selected) will drastically reduce the number of requests required to update the cache. Use in combination with `--refresh-cache-jcl` or `--refresh-cache-lcsc` flags

`./lcsc-scrape fetch --purge-cached-errors` - this will delete the cached copy if no useful data can be extracted from it. This can be run against the full spreadsheet without too many extra requests - in my initial run < 100 requests failed and of those only 20odd gave HTTP status 200 but broken/missing data that worked later. This flag isn't the default because there are products in the list that don't have product pages on LCSC (they show up in search but there's nothing to click on) and I didn't want to keep searching for those known missing/defunct products.

`./lcsc-scrape fetch --refresh-cache-jlc` - this will ignore cached data when getting data from JLCPCB. This is useful if you want up to date stock/price info. The JLCPCB api is much faster than LCSC so doing this doesn't take that long. The risk of being banned is still there of course.

## Querying 

Columns beginning with pp_ are the post processed ones and are BOOL or REAL 

`./lcsc-scrape query --drop-null-columns "SELECT ..."` - this executes the provided SQL and outputs the results as comma separated values to STDOUT.

If there are slow queries you are running regularly you can use query to add indexes. I've not found it necessary as the whole DB fits in memory and all queries complete in < 1 second for me so far.

### Useful queries

Get a list of LCSC categories to filter by
```
./lcsc-scrape query "SELECT DISTINCT Category FROM parts ORDER BY Category"
```


Get a list of non empty columns for the given category without all the extra Price columns
```
./lcsc-scrape query --drop-null-columns 'SELECT * FROM parts WHERE Category = "Power Management ICs/DC-DC Converters"' | head -n 1 | tr ',' '\n' | grep -v "Price"
```


Get all DC-DC converters with < 5V input OR NULL input from the JLC Basic parts library. NULL is included so you know which parts you need to manually check the datasheets for.

```
./lcsc-scrape query --drop-null-columns \
'SELECT part_number, Basic, "Voltage - Input (Min)"
FROM parts 
WHERE Category = "Power Management ICs/DC-DC Converters"
    AND pp_Basic = 1 
    AND ("pp_Voltage - Input (Min)" <= 5 OR 
        "pp_Voltage - Input (Min)" is null)
    ORDER BY "pp_Voltage - Input (Min)" DESC'
```

Get the descriptions and datasheet links for a part when the key value pairs aren't useful
```
./lcsc-scrape query \
"SELECT part_number, Description, JLC_Description, Datasheet, JLC_Datasheet"
FROM parts 
WHERE part_number = "C14867"'
```

## License

Free and open source software distributed under the terms of both the [MIT License][lm] and the [Apache License 2.0][la].

[lm]: LICENSE-MIT
[la]: LICENSE-APACHE