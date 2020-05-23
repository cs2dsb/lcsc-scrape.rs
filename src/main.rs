use std::{
    fs,
    io::{
        Error as IoError,
        Write,
        Read,
    },
    env,
    path::{ Path },
    collections::{
        HashMap,
        //HashSet,
    },
    time::Instant,
};

// LinkedHashSet preserves insertion order
use linked_hash_set::LinkedHashSet as HashSet;
use serde::{ Deserialize, Serialize };
use serde_json::Error as SerdeJsonError;
use env_logger;
use log::*;
use lazy_static::lazy_static;
use reqwest::{
    Error as ReqwestError,
};
use clap::{
    crate_version, 
    crate_name,
    App,
    AppSettings,
    SubCommand,
    Arg,
    ArgGroup,
};
use regex::{
    Regex, 
    Error as RegexError,
};
use scraper::{
    Html,
    Selector,
    element_ref::{
        Select,
        ElementRef,
    },
};
use rusqlite::{
    Connection,
    Error as DbError,
    NO_PARAMS,
    Row,
    types::{
        Type as SqlType,
        ValueRef,
    },
};

use calamine::{
    Reader, 
    open_workbook_auto,
    Error as SheetError,
    DataType,
};

lazy_static! {
    static ref REGEX_PN: Regex = Regex::new(r"^[cC]\d+(_b)*$").unwrap();
    static ref REGEX_VOLTAGE: Regex = Regex::new(r"^(-{0,1}\d+(\.\d+)*)(V|Vin|mV)").unwrap();
    static ref REGEX_PRICE: Regex = Regex::new(r"^(?:US\$){0,1}(\d+(\.\d+){0,1})$").unwrap();
    static ref REGEX_RESISTANCE: Regex = Regex::new(r"^(\d+(\.\d+){0,1})[KMmun]*$").unwrap();

    static ref SELECTOR_INFO_TABLE: Selector = Selector::parse("table.info-table").unwrap();
    static ref SELECTOR_SPEC_TABLE: Selector = Selector::parse("table.products-specifications").unwrap();
    static ref SELECTOR_PRICE_TABLE: Selector = Selector::parse("table.main-table").unwrap();
    static ref SELECTOR_TABLE_BODY_ROW: Selector = Selector::parse("tbody > tr").unwrap();
    static ref SELECTOR_TD: Selector = Selector::parse("td").unwrap();
    static ref SELECTOR_ORIGINAL_PRICE: Selector = Selector::parse("span.origin-price").unwrap();

    static ref SELECTOR_ROHS: Selector = Selector::parse("span.rohs").unwrap();
    static ref SELECTOR_LINK: Selector = Selector::parse("a").unwrap();
}

const RESISTANCE_SUFFIX: [(&'static str, f64); 5] = [
    ("K", 1.0E3),
    ("M", 1.0E6),
    ("m", 1.0E-3),
    ("u", 1.0E-6),
    ("n", 1.0E-9),
];

#[derive(Debug)]
enum MainErrors {
    ConfigError(&'static str),
    IoError(IoError),
    RegexError(RegexError),
    ValidationError(String),
    ReqwestError(ReqwestError),
    SerdeJsonError(SerdeJsonError),
    DbError(DbError),
    SheetError(SheetError),
    ResponseError(String),
}

impl From<IoError> for MainErrors {
    fn from(e: IoError) -> MainErrors {
        MainErrors::IoError(e)
    }
}

impl From<RegexError> for MainErrors {
    fn from(e: RegexError) -> MainErrors {
        MainErrors::RegexError(e)
    }
}

impl From<ReqwestError> for MainErrors {
    fn from(e: ReqwestError) -> MainErrors {
        MainErrors::ReqwestError(e)
    }
}

impl From<SerdeJsonError> for MainErrors {
    fn from(e: SerdeJsonError) -> MainErrors {
        MainErrors::SerdeJsonError(e)
    }
}

impl From<DbError> for MainErrors {
    fn from(e: DbError) -> MainErrors {
        MainErrors::DbError(e)
    }
}

impl From<SheetError> for MainErrors {
    fn from(e: SheetError) -> MainErrors {
        MainErrors::SheetError(e)
    }
}


#[derive(Debug, Deserialize, Serialize)] 
pub struct SearchResponse {
    success: bool,
    message: String,
    result: SearchResult,
}

#[derive(Debug, Deserialize, Serialize)] 
pub struct SearchResultLink {
    lcsc_part_number: bool,
    links: String,
}

#[derive(Debug, Deserialize, Serialize)] 
#[serde(untagged)]
pub enum SearchResult {
    Link(SearchResultLink),
    // If there is no exact match it returns an empty array for 
    // whatever reason
    Array(Vec<String>),
}

impl SearchResult {
    fn is_link(&self) -> bool {
        match self {
            SearchResult::Link(_) => true,
            _ => false,
        }
    }

    fn get_link(self) -> SearchResultLink {
        match self {
            SearchResult::Link(l) => l,
            _ => unreachable!(),
        }
    }
}


#[derive(Debug, Deserialize, Serialize)] 
#[serde(rename_all = "camelCase")]
pub enum JlcComponentLibraryType {
    Expand,
    Basic,
    Base,
}

impl Default for JlcComponentLibraryType {
    fn default() -> JlcComponentLibraryType {
        JlcComponentLibraryType::Basic
    }
}

impl JlcComponentLibraryType {
    fn is_basic(&self) -> bool {
        match self {
            JlcComponentLibraryType::Expand => false,
            _ => true,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Default)] 
#[serde(rename_all = "camelCase")]
pub struct JlcComponentPrice {
    product_price: f64,
    // Price break ranges
    start_number: isize,
    end_number: isize,
}

#[derive(Debug, Deserialize, Serialize, Default)] 
#[serde(rename_all = "camelCase")]
pub struct JlcSmtCompListItem {
    stock_count: isize,
    component_code: String,
    component_library_type: JlcComponentLibraryType,
    #[serde(default)]
    component_prices: Vec<JlcComponentPrice>,
    describe: Option<String>,
    data_manual_url: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Default)] 
pub struct JlcSmtCompListData {
    total: isize,
    list: Vec<JlcSmtCompListItem>,      
}

#[derive(Debug, Deserialize, Serialize, Default)] 
pub struct JlcSmtCompListResult {
    code: isize,
    data: JlcSmtCompListData,
    message: Option<String>,
}

impl JlcSmtCompListResult {
    fn merge(&mut self, other: Self) {
        let JlcSmtCompListResult { code, data, message } = other;
        let JlcSmtCompListData { total, list } = data;
        
        self.code = code;
        self.data.list.extend(list);
        self.data.total = self.data.total.max(total);

        if let Some(o_msg) = message {
            if let Some(msg) = &mut self.message {
                msg.push_str("\n");
                msg.push_str(&o_msg);
            } else {
                self.message = Some(o_msg);
            }
        }

    }
}



fn create_db(con: &mut Connection) -> Result<(), DbError> {
    con.execute(r"
        create table if not exists parts (
            part_number text primary key
        )",
        NO_PARAMS,
    )?;

    con.execute(r"
        create table if not exists keys (
            key text primary key 
        )",
        NO_PARAMS,
    )?;

    Ok(())
}

fn populate_columns(con: &Connection, columns: &mut Vec<String>) -> Result<(), MainErrors> {
    columns.clear();

    let mut stmt = con.prepare(r"
        SELECT key
        FROM keys")?;

    let mut rows = stmt.query(NO_PARAMS)?;
    while let Some(r) = rows.next()? {
        columns.push(r.get(0)?);
    }

    Ok(())
}

trait ToDbType {
    const TYPE_STRING: &'static str;
}

impl ToDbType for String {
    const TYPE_STRING: &'static str = "text";
}

impl ToDbType for bool {
    const TYPE_STRING: &'static str = "integer";
}

impl ToDbType for f64 {
    const TYPE_STRING: &'static str = "real";
}

fn execute_and_log_err(con: &Connection, stmt: &str) -> Result<usize, DbError> {
    trace!("{}", stmt);
    let res = con.execute(stmt, NO_PARAMS);
    if res.is_err() {
        error!("Error executing db statment: {}", stmt);
    }
    res
}

fn add_column_type<T: ToDbType>(con: &Connection, columns: &mut Vec<String>, column: String) -> Result<(), MainErrors> {
    if columns.contains(&column) {
        return Ok(());
    }

    execute_and_log_err(con, &format!("alter table parts add column \"{}\" {}", &column, T::TYPE_STRING))?;
    execute_and_log_err(con, &format!("insert into keys (key) values (\"{}\")", &column))?;

    columns.push(column);

    Ok(())
}

fn add_column(con: &Connection, columns: &mut Vec<String>, column: String) -> Result<(), MainErrors> {
    add_column_type::<String>(con, columns, column)
}

fn extract_row_kvp(select: Select) -> Result<Option<(String, String)>, ()> {
    let tds: Vec<ElementRef> = select.collect();

    if tds.len() < 2 {
        return Err(());
    }

    let flatten = |e: &ElementRef| {
        let mut text = String::new();
        for t in e.text() {
            for c in t.trim().chars().filter(|x| x.is_ascii()) {
                text.push(c);
            }
        }
        text
    };

    let key = flatten(&tds[0]);
    let mut value = flatten(&tds[1]);

    // They use "-" and " " interchangeably for no value
    if value == "-" {
        return Ok(None);
    }

    if key == "RoHS" {
        if tds[1].select(&SELECTOR_ROHS).count() > 0 {
            value = "true".to_string();
        } else {
            value = "false".to_string();
        }
    } else if key == "Datasheet" {
        for link in tds[1].select(&SELECTOR_LINK) {
            if let Some(link) = link.value().attr("href") {
                value = link.to_string();
            }
        }
    } else if key == "EasyEDA Libraries" {
        // Link is loaded via XHR so it's not available in our static scrape
        return Ok(None);
    } else if key == "Customer #" {
        return Ok(None);
    } else if value.matches('$').count() > 1 {
        // Discounted items result in two prices munged together otherwise
        trace!("IH: {}", tds[1].inner_html());
        for original in tds[1].select(&SELECTOR_ORIGINAL_PRICE) {
            value = flatten(&original);
        }
    }

    Ok(Some((key, value)))
}

fn extract_table_kvp(part_number: &str, html: &Html, selector: &Selector) -> Vec<(String, String)> {
    let mut results = Vec::new();

    for table in html.select(selector) {
        for (row_i, row) in table.select(&SELECTOR_TABLE_BODY_ROW).enumerate() {
            
            match extract_row_kvp(row.select(&SELECTOR_TD)) {
                Ok(Some(pair)) => results.push(pair),
                Ok(None) => {},
                Err(_) => error!("{}: Failed to extract key & value from row {}", part_number, row_i),
            }
        }
    }

    results
}

fn download_part(part_number: &str) -> Result<String, MainErrors> {
    let client = reqwest::blocking::Client::new();

    let url = format!("https://lcsc.com/api/global/additional/search?q={}", part_number);
    debug!("{}: Fetching LCSC page ({})", part_number, url);

    let res = client
        .get(&url)
        .send()?;

    let st = res.status();
    if !st.is_success() {
        Err(MainErrors::ResponseError(format!("Failed to find product link (HTTP code {:?})",  st)))?;
    }

    let text = res.text()?;

    let res: SearchResponse = serde_json::from_str(&text)?;
    if !res.success || !res.result.is_link() {
        debug!("Non sucess/link response data: {:#?}", res);
        Err(MainErrors::ResponseError(format!("Failed to find product link (success {}, is_link {}) - usually means the produc detail page has been deleted or never existed", res.success, res.result.is_link())))?;
    }

    let link = format!("https://lcsc.com{}", res.result.get_link().links);
    debug!("{}: Product link = {}", part_number, link);

    let res = client
        .get(&link)
        .send()?;

    let st = res.status();
    if !st.is_success() {
        Err(MainErrors::ResponseError(format!("Failed to get product page (HTTP code {:?})",  st)))?;
    }

    let text = res.text()?;

    Ok(text)
}

fn download_jlc_info(part_number: &str) -> Result<String, MainErrors> {
    let client = reqwest::blocking::Client::new();

    let url = "https://jlcpcb.com/shoppingCart/smtGood/selectSmtComponentList";

    let mut data = HashMap::new();
    let mut current_page = 1;
    data.insert("pageSize", "10".to_string());
    data.insert("searchSource", "search".to_string());
    data.insert("keyword", part_number.to_string());

    let mut merged_result = JlcSmtCompListResult::default();
    let mut more = true;

    while more {
        debug!("{}: Fetching JLC info page {} ({})", part_number, current_page, url);

        data.insert("currentPage", format!("{}", current_page));

        let res = client.post(url)
            .header("Accept", "application/json")
            .form(&data)
            .send()?;

        let st = res.status();
        if !st.is_success() {
            Err(MainErrors::ResponseError(format!("Failed to get JLC data (HTTP code {:?})",  st)))?;
        }

        let text = res.text()?;

        let res = serde_json::from_str::<JlcSmtCompListResult>(&text)?;
        if res.code != 200 {
            debug!("Non 200 response data: {:#?}", res);
            Err(MainErrors::ResponseError(format!("Failed to get JLC data (code {})", res.code)))?;
        }

        merged_result.merge(res);

        more = merged_result.data.list.len() < (merged_result.data.total as usize);
        current_page += 1;
    }

    Ok(serde_json::to_string(&merged_result)?)
}

fn save_pairs(con: &Connection, columns: &mut Vec<String>, part_number: &str, pairs: Vec<(String, String)>) -> Result<(), MainErrors> {
    if pairs.len() == 0 {
        return Ok(());
    }

    let mut new_columns = Vec::new();
    for (key, _) in pairs.iter() {
        if !columns.contains(key) {
            new_columns.push(key.clone());
        }
    }

    for nc in new_columns {
        add_column(con, columns, nc)?;
    }

    let exists = {
        let mut stmt = con.prepare("SELECT EXISTS(SELECT 1 FROM parts WHERE part_number = ?1)")?;
        let mut rows = stmt.query(&[part_number])?;
        let r = match rows.next()? {
            Some(r) => r.get(0)?,
            None => 0,
        };
        r == 1
    };

    let mut statement = String::new();
    if exists { 
        statement.push_str("UPDATE parts SET ");
        for (i, (key, value)) in pairs.iter().enumerate() {
            if i != 0 {
                statement.push_str(", ");
            }
            // Remove any quotes in the string
            let value = value.replace("\"", "");
            statement.push_str(&format!("\"{}\"=\"{}\" ", key, value));
        }        
        statement.push_str(&format!(" WHERE part_number = \"{}\"", part_number));
    } else {
        statement.push_str("INSERT INTO parts (part_number");
        for (key, _) in pairs.iter() {
            statement.push_str(&format!(", \"{}\"", key));
        }
        statement.push_str(&format!(") values (\"{}\"", part_number));
        for (_, value) in pairs {
            // Remove any quotes in the string
            let value = value.replace("\"", "");
            
            statement.push_str(&format!(", \"{}\"", value));
        }        
        statement.push_str(")");
    }

    execute_and_log_err(con, &statement)?;

    Ok(())
}

fn fetch(
    con: &mut Connection, 
    cache_path: &Path, 
    part_numbers: Vec<String>, 
    jlc_data: bool, 
    refresh_cache_jlc: bool, 
    refresh_cache_lcsc: bool,
    download_to_cache_only: bool,
    purge_cached_errors: bool,
) -> Result<(), MainErrors> {
    // Do the validation first so it doesn't explode half way through after fetching a bunch
    for part in part_numbers.iter() {
        if !REGEX_PN.is_match(part) {
            Err(MainErrors::ValidationError(format!("Part number {} doesn't match expected pattern {}", 
                part, REGEX_PN.to_string())))?; 
        }
    }

    let tx = con.transaction()?;

    let mut columns = Vec::new();
    populate_columns(&tx, &mut columns)?;

    let num_parts = part_numbers.len();
    let ten_pc = num_parts / 10;
    let mut downloaded = 0;
    let mut cached = 0;
    let mut purged = 0;

    for (i, mut part) in part_numbers.into_iter().enumerate() {
        let mut basic = false;
        if part.ends_with("_b") {
            basic = true;
            part.truncate(part.len() - 2);
        }

        if i % ten_pc == 0 {
            info!("Total {:8}, Processed: {:8} ({:3}%), Downloaded: {:8}, Cached: {:8}, Purged: {:8}", 
                num_parts,
                i,
                (i / ten_pc) * 10,
                downloaded,
                cached,
                purged,
            );
        }


        let mut pairs = Vec::new();

        // Pairs are essentially in priority order since the hashmap won't take the 2nd value after the first is added
        // So jlc data is first to make sure Basic reflects that if it's there
        if jlc_data {
            let path = cache_path.join(format!("{}_jlc_data", part));
            if refresh_cache_jlc || !path.exists() || fs::metadata(path.clone())?.len() == 0 {
                match download_jlc_info(&part) {
                    Ok(text) => {
                        let mut file = fs::File::create(path.clone())?;
                        file.write_all(text.as_bytes())?;
                        downloaded += 1;
                    },
                    Err(e) => error!("{}: Error fetching JLC part info: {:?}", part, e),
                };                
            } else {
                debug!("{}: Cached (JLC)", part); 
                cached += 1;
            }

            if !download_to_cache_only && path.exists() {
                let info = {
                    let mut file = fs::File::open(&path)?;
                    let mut contents = String::new();
                    file.read_to_string(&mut contents)?;
                    serde_json::from_str::<JlcSmtCompListResult>(&contents)
                };

                let mut purge = purge_cached_errors;

                match info {
                    Err(e) => error!("{}: Error parsing JLC part info response: {:?}", part, e),
                    Ok(i) => {
                        if i.code != 200 {
                            error!("{}: Parsed JLC part info response code was not 200: {:?}", part, i);
                        } else {
                            if let Some(item) = i.data.list
                                .iter()
                                .filter(|x| x.component_code == part)
                                .nth(0)
                            {
                                basic = item.component_library_type.is_basic();
                                let stock = item.stock_count;

                                pairs.push(("JLC_Stock".to_string(), format!("{}", stock)));

                                if let Some(description) = &item.describe {
                                    pairs.push(("JLC_Description".to_string(), description.clone()));
                                }
                                if let Some(datasheet) = &item.data_manual_url {
                                    pairs.push(("JLC_Datasheet".to_string(), datasheet.clone()));
                                }

                                for price in item.component_prices.iter() {
                                    pairs.push((
                                        format!("Price JLC {}+", price.start_number),
                                        format!("{}", price.product_price)));
                                }

                                purge = false;
                            } else {
                                debug!("Missing part response data: {:#?}", i);
                                error!("{}: Parsed JLC part info but results didn't contain the part number we're looking for", part);
                            }
                        }
                    }
                }

                if purge {
                    info!("{}: Purging jlc data cache file", part);
                    fs::remove_file(path)?;
                    purged += 1;
                }
            }
        }


        let path = cache_path.join(&part);
        if refresh_cache_lcsc || !path.exists() || fs::metadata(path.clone())?.len() == 0 {
            let text = match download_part(&part) {
                Ok(text) => text,
                Err(e) => {
                    error!("{}: Error fetching LCSC part: {:?}", part, e);
                    continue;
                },
            };
            let mut file = fs::File::create(path.clone())?;
            file.write_all(text.as_bytes())?;
            downloaded += 1;
        } else {
            debug!("{}: Cached (LCSC)", part); 
            cached += 1;
        }

        if download_to_cache_only {
            continue;
        }

        let html = {
            let mut file = fs::File::open(&path)?;
            let mut contents = String::new();
            file.read_to_string(&mut contents)?;
            Html::parse_document(&contents)
        };

        let mut purge = purge_cached_errors;

        let pcount = pairs.len();

        pairs.extend(extract_table_kvp(&part, &html, &SELECTOR_SPEC_TABLE).into_iter());
        pairs.extend(extract_table_kvp(&part, &html, &SELECTOR_INFO_TABLE).into_iter());
        pairs.extend(extract_table_kvp(&part, &html, &SELECTOR_PRICE_TABLE).into_iter()
            .map(|(key, value)| (format!("Price {}", key), value)));

        if pcount < pairs.len() {
            purge = false;
        }

        pairs.push(("Basic".to_string(), format!("{}", basic)));

        if purge {
            info!("{}: Purging lcsc cache file", part);
            fs::remove_file(path)?;
            purged += 1;
        }


        let dedup: HashMap<_, _> = pairs.into_iter().collect();
        let pairs: Vec<(String, String)> = dedup.into_iter().collect();

        save_pairs(&tx, &mut columns, &part, pairs)?;
    }

    info!("Total {:8}, Processed: {:8} ({:3}%), Downloaded: {:8}, Cached: {:8}, Purged: {:8}", 
        num_parts,
        num_parts,
        100,
        downloaded,
        cached,
        purged,
    );

    tx.commit()?;

    Ok(())
}

fn get_and_log_err(row: &Row, n: usize, stmt: &str) -> Result<Option<String>, DbError> {
    let res = row.get(n);
    match res {
        Ok(s) => Ok(s),
        Err(e) => {
            error!("Error getting col {} out of row from statement: {}\n{:?}", n, stmt, e);
            Err(e)
        }
    }
}

fn run_post_process<F, T>(con: &Connection, columns: &mut Vec<String>, column: &str, f: F) -> Result<(), MainErrors> 
where 
    // Result string is NOT quoted when creating the update statement to allow numbers/etc. 
    // if result is actually a string, quotes need to be included in the result
    F: Fn(Option<String>, &str) -> Option<String>,
    T: ToDbType,
{
    info!("Post processed \"{}\" column ", column);

    let pp_column = format!("pp_{}", column);
    
    add_column_type::<T>(con, columns, pp_column.clone())?;

    let stmt = &format!("SELECT part_number, \"{}\" FROM parts", column);
    trace!("{}", stmt);
    let mut prep = con.prepare(stmt)?;
    let res = prep.query(NO_PARAMS);
    if res.is_err() {
        error!("Error executing query statement: {}", stmt);
    }
    let mut rows = res?;

    while let Some(r) = match rows.next() {
        Ok(r) => r,
        Err(e) => {
            error!("Error executing statement: \n{}\n{:?}", stmt, e);
            return Err(e)?;
        }
    } {
        let part_number: String = get_and_log_err(r, 0, stmt)?.unwrap();
        let input: Option<String> = get_and_log_err(r, 1, stmt)?;

        if let Some(output) = f(input, &part_number) {
            let stmt = format!("
                UPDATE parts SET \"{}\" = {}
                WHERE part_number = \"{}\"",
                pp_column,
                output,
                part_number);


            let res = con.execute(&stmt, NO_PARAMS);
            if res.is_err() {
                error!("Error executing statement: \n{}\n\n", stmt);
            }
            res?;
        }
    }


    Ok(())
}


// Checks the value against the regex then parses capture group 1 as a f64
fn parse_float(value: &str, regex: &Regex, column: &str, part_number: &str) -> Option<f64> {
    if let Some(caps) = regex.captures(value) {
        if let Some(f_match) = caps.get(1) {
            return match f_match.as_str().parse::<f64>() {
                Ok(p) => Some(p),
                Err(e) => {
                    warn!("{}: Found value \"{}\" in column \"{}\" but failed to parse it as a float: {:?}", 
                        part_number, f_match.as_str(), column, e);
                    None
                }
            };
        } 
    } 

    warn!("{}: Found value \"{}\" in column \"{}\" but failed to parse match against regex \"{}\"", 
        part_number, value, column, regex.as_str());
    None
}

// Calls parse_float then converts the result back to a string ready to be inserted 
// in the db. Could probably just stuff the regex capture in the field but I'd rather
// it fail to parse here and raise a warning than SQL queries silently not matching later
fn parse_float_to_db_str(value: &str, regex: &Regex, column: &str, part_number: &str) -> Option<String> {
    parse_float(value, regex, column, part_number)
        .map(|x| format!("{}", x))
}

fn post_process(con: &mut Connection) -> Result<(), MainErrors> {
    let tx = con.transaction()?;

    let mut columns = Vec::new();
    populate_columns(&tx, &mut columns)?;

    
    let filtered_columns: Vec<String> = columns
        .iter()
        .filter_map(|x| if x.starts_with("pp_") {
            None
        } else {
            Some(x.clone())
        })
        .collect();

    run_post_process::<_, bool>(&tx, &mut columns, "Basic", |value, part_number| {
        if value.is_none() { return None; }
        let value = value.unwrap();
        match value.as_str() {
            "true" => Some("1".to_string()),
            "false" => Some("0".to_string()),
            o => {
                warn!("{}: Found value \"{}\" in column \"Basic\", expected only {{true, false}}", part_number, o);
                None
            },
        }
    })?;

    run_post_process::<_, f64>(&tx, &mut columns, "Resistance (Ohms)", |value, part_number| {
        if value.is_none() { return None; }
        let value = value.unwrap();
        
        parse_float(&value, &REGEX_RESISTANCE, "Resistance (Ohms)", part_number).map(|x| {
            let mut mul = 1.;
            for (suffix, m) in RESISTANCE_SUFFIX.iter() {
                if value.ends_with(suffix) {
                    mul = *m;
                    break;
                }
            }
            format!("{}", x * mul)
        })
    })?;
    
    //These loops are separate so that errors for a given post-process are grouped together rather than
    //mixed in the order that the columns are in the DB
    for col in filtered_columns.iter() {
        if col.starts_with("Price") {
            run_post_process::<_, f64>(&tx, &mut columns, col, |value, part_number| {
                if value.is_none() { return None; }
                let value = value.unwrap();

                parse_float_to_db_str(&value, &REGEX_PRICE, col, part_number)
            })?;
        }
    }

    for col in filtered_columns.iter() {
        if col.contains("Voltage") {
            run_post_process::<_, f64>(&tx, &mut columns, col, |value, part_number| {
                if value.is_none() { return None; }
                let value = value.unwrap();

                parse_float_to_db_str(&value, &REGEX_VOLTAGE, col, part_number)
            })?;
        }
    }

    tx.commit()?;
    

    Ok(())
}

fn clear_database(con: &mut Connection) -> Result<(), MainErrors> {
    execute_and_log_err(con, "DROP TABLE IF EXISTS keys")?;
    execute_and_log_err(con, "DROP TABLE IF EXISTS parts")?;
    execute_and_log_err(con, "VACUUM")?;
    info!("Database cleared");
    Ok(())
}

fn clear_cache(cache_path: &Path) -> Result<(), MainErrors> {
    if cache_path.exists() {
        fs::remove_dir_all(cache_path)?
    }
    info!("Cache cleared");
    Ok(())
}

fn csv<'a, I>(i: I)
where 
    I: Iterator<Item=&'a String>
{
    for (i, v) in i.enumerate() {
        if i > 0 {
            print!(", ");
        } 
        print!("{}", v);
    }
    println!("");
}

fn query(con: &mut Connection, query: &str, drop_null_columns: bool) -> Result<(), MainErrors> {
    let start = Instant::now();

    let mut prep = con.prepare(query)?;
    let res = prep.query(NO_PARAMS);
    if res.is_err() {
        error!("Error executing query statement: {}", query);
    }
    let mut rows = res?;

    let mut columns = HashSet::new();
    let mut row_maps: Vec<HashMap<String, String>> = Vec::new();

    while let Some(r) = rows.next()? {
        let mut row_map = HashMap::new();

        for i in 0..r.column_count() {
            let name = r.column_name(i)?;
            let v = r.get_raw_checked(i)?;

            if (v.data_type() != SqlType::Null || !drop_null_columns) && !columns.contains(name) {
                columns.insert(name.to_string());
            }

            let v = match v {
                ValueRef::Null => "".to_string(),
                ValueRef::Blob(_) => "[BLOB]".to_string(),
                ValueRef::Integer(v) => format!("{}", v),
                ValueRef::Real(v) => format!("{}", v),
                ValueRef::Text(v) => format!("{}", std::str::from_utf8(v)
                    .map_err(|e| DbError::Utf8Error(e))?),
            };

            row_map.insert(name.to_string(), v);
        }

        row_maps.push(row_map);
    }

    csv(columns.iter());
    for r in row_maps.iter() {
        let mut values = Vec::new();
        for c in columns.iter() {
            values.push(r.get(c).unwrap());
        }
        csv(values.into_iter());
    }

    info!("Query took {:?}", start.elapsed());

    Ok(())
}

fn main() -> Result<(), MainErrors> {
    let matches = App::new(crate_name!())
        .version(crate_version!())
        .about("Fetch LCSC part numbers into a local sqlite db and query that db")
        .setting(AppSettings::SubcommandRequiredElseHelp)
        .global_setting(AppSettings::ColoredHelp)
        .subcommand(SubCommand::with_name("fetch")
            .about("Fetch part info from LCSC and store it locally")
            .arg(Arg::with_name("parts")
                .help("Space separated list of lcsc part numbers to fetch. \"_b\" on the end of the part number will mark it as a basic part")
                .long("parts")
                .short("p")
                .multiple(true)
                .takes_value(true))
            .arg(Arg::with_name("parts-list")
                .help("Path to a file containing the parts list. Space or newline delimited.  \"_b\" on the end of the part number will mark it as a basic part")
                .long("parts-list")
                .short("l")
                .takes_value(true))
            .arg(Arg::with_name("parts-spreadsheet")
                .help("Path to the parts spreadhseet from JLC's website. Looks for \"LCSC Part\" and \"Library Type\" columns containing the parts list. Note, the file direct from JLC won't open correctly, converting it to ods or xlsx fixes it (and reduces the file size by 10x for some reason)")
                .long("parts-spreadsheet")
                .short("s")
                .takes_value(true))
            .group(ArgGroup::with_name("part-spec")
                .args(&["parts", "parts-list", "parts-spreadsheet"])
                .required(true))
            .arg(Arg::with_name("jlc-data")
                .help("Also fetch stock, price and basic/extended info")
                .long("jlc-data")
                .short("j")
                .takes_value(false))
            .arg(Arg::with_name("refresh-cache-jlc")
                .help("Fetch the jlc-data from JLC rather than cache. Beware of getting banned for excessive requests maybe???")
                .long("refresh-cache-jlc")
                .takes_value(false))
            .arg(Arg::with_name("refresh-cache-lcsc")
                .help("Fetch the lcsc info from JLC rather than cache. Beware of getting banned for excessive requests maybe???")
                .long("refresh-cache-lcsc")
                .takes_value(false))
            .arg(Arg::with_name("download-to-cache-only")
                .help("Fetch lcsc (and jlc if jlc-data flag is set) to the local cache if not already there but don't update the database. Works with refresh-cache-*")
                .long("download-to-cache-only")
                .takes_value(false))
            .arg(Arg::with_name("purge-cached-errors")
                .help("If a cached file fails to parse (json errors, missing results, etc.) delete the file. Next run will fetch a new copy. Leave off to prevent repeated scraping of products with actual missing data on LCSC/JLC")
                .long("purge-cached-errors")
                .takes_value(false)))
        .subcommand(SubCommand::with_name("manage")
            .setting(AppSettings::SubcommandRequiredElseHelp)
            .about("Manage the local cache and database")
            .subcommand(SubCommand::with_name("clear-database")
                .about("Delete everything in the database, doesn't touch the download cache"))
            .subcommand(SubCommand::with_name("clear-cache")
                .about("Delete everything in the cache directory, doesn't touch the database"))
            .subcommand(SubCommand::with_name("post-process")
                .about("Post process the contents of the database to parse various fields to numbers/booleans. The log will show the columns that are added")))
        .subcommand(SubCommand::with_name("query")
            .about("Query the database")
            .arg(Arg::with_name("drop-null-columns")
                .help("Doesn't print columns with all NULL values")
                .long("drop-null-columns")
                .takes_value(false))
            .arg(Arg::with_name("query")
                .help("The sqlite SQL statement to execute")
                .takes_value(true)))
        .arg(Arg::with_name("cache_path")
            .default_value("./cache")
            .help("Path to the local cache directory where downloaded LCSC pages are stored")
            .long("cache-path")
            .short("c")
            .takes_value(true))
        .arg(Arg::with_name("database")
            .default_value("lcsc.sqlite")
            .help("Path to the local database file")
            .long("database")
            .short("d")
            .takes_value(true))
        .arg(Arg::with_name("verbose")
            .help("Increase the verbosity (-v = debug, -vv = trace)")
            .long("verbose")
            .short("v")
            .multiple(true)
            .takes_value(false))
        .get_matches();

    // Sort out logging verbosity first
    if env::var("RUST_LOG").is_err() {
        let level = match matches.occurrences_of("verbose") {
            0 => "info",
            1 => "debug",
            _ => "trace",
        };
        env::set_var("RUST_LOG", format!("lcsc_scrape={}", level));
    }
    env_logger::init();

    // The database path is always required
    let db = matches
        .value_of("database")
        .ok_or_else(|| MainErrors::ConfigError("Missing required argument \"database\""))?;
    let mut con = Connection::open(db)?;
    create_db(&mut con)?;

    // The cache path is always required
    let cache_path = matches
        .value_of("cache_path")
        .ok_or_else(|| MainErrors::ConfigError("Missing required argument \"cache-path\""))?
        .to_owned();
    let cache_path = Path::new(&cache_path);

    // Create the cache dir if it doesn't exist
    fs::create_dir_all(cache_path)?;

    if let Ok(canon) = cache_path.canonicalize() {
        info!("Cache dir: {:?}", canon);
    } else {
        info!("Cache dir: {:?}", cache_path);
    }

    let (name, args) = matches.subcommand();
    match name {
        "fetch" => {
            let mut part_numbers = Vec::new();

            // Unwrap is safe here because clap is requiring one or other of these args
            let args = args.unwrap();
            if let Some(parts) = args.values_of("parts") {
                for p in parts {
                    part_numbers.push(p.to_string());
                }
            }
            if let Some(parts_list) = args.value_of("parts-list") {
                let path = Path::new(parts_list);
                if !path.exists() {
                    Err(MainErrors::ConfigError("Supplied parts-list file path doesn't exist"))?;
                }
                let mut file = fs::File::open(path)?;
                let mut contents = String::new();
                file.read_to_string(&mut contents)?;

                for p in contents.split_whitespace() {
                    part_numbers.push(p.to_string());
                }                    
            }
            if let Some(parts_spreadsheet) = args.value_of("parts-spreadsheet") {
                let path = Path::new(parts_spreadsheet);
                if !path.exists() {
                    Err(MainErrors::ConfigError("Supplied parts-spreadsheet file path doesn't exist"))?;
                }

                info!("Parsing spreadhseet");
                let mut workbook = open_workbook_auto(path).unwrap();
                for s in workbook.sheet_names().to_owned() {
                    if let Some(Ok(range)) = workbook.worksheet_range(&s) {
                        let mut part_number_i = None;
                        let mut basic_i = None;

                        if let Some(r1) = range.rows().nth(0) {
                            for (i, c) in r1.iter().enumerate() {
                                if let DataType::String(ref s) = c {
                                    if s == "LCSC Part" {
                                        part_number_i = Some(i as u32);
                                    } else if s == "Library Type" {
                                        basic_i = Some(i as u32);
                                    }
                                }
                            }
                        }

                        if let (Some(part_number_i), Some(basic_i)) = (part_number_i, basic_i) {
                            for ri in 1..(range.get_size().0 as u32) {
                                let part_number = range.get_value((ri, part_number_i));
                                let basic = range.get_value((ri, basic_i));

                                if let (Some(DataType::String(pn)), Some(DataType::String(b))) = (part_number, basic) {
                                    let mut pn = pn.to_string();
                                    // Sometimes it says Basic sometimes base
                                    if b.to_lowercase().contains("bas") {
                                        pn.push_str("_b");
                                    }

                                    part_numbers.push(pn);
                                } else {
                                    info!("{:?} {:?}", part_number, basic);
                                }
                            }
                        }
                    }
                }
            }

            if part_numbers.len() == 0 {
                Err(MainErrors::ConfigError("No part numbers were provided"))?;
            }

            let refresh_cache_jlc = args.is_present("refresh-cache-jlc");
            let refresh_cache_lcsc = args.is_present("refresh-cache-lcsc");
            let download_to_cache_only = args.is_present("download-to-cache-only");
            let jlc_data = refresh_cache_jlc || args.is_present("jlc-data");
            let purge_cached_errors = args.is_present("purge-cached-errors");

            fetch(&mut con, cache_path, part_numbers, jlc_data, refresh_cache_jlc, refresh_cache_lcsc, download_to_cache_only, purge_cached_errors)?
        },
        // Unwrap is safe because clap is requiring a subcommand
        "manage" => match args.unwrap().subcommand() {
            ("post-process", _) => post_process(&mut con)?,
            ("clear-database", _) => clear_database(&mut con)?,
            ("clear-cache", _) => clear_cache(cache_path)?,
            _ => {},
        },
        "query" => {
            let args = args.unwrap();
            query(
                &mut con, 
                args.value_of("query").unwrap(),
                args.is_present("drop-null-columns"),
            )?;
        },
        _ => (),
    }

    info!("Finished OK");

    Ok(())
}

trait TrimInPlace { fn trim_in_place (self: &'_ mut Self); }
impl TrimInPlace for String {
    fn trim_in_place (self: &'_ mut Self)
    {
        let (start, len): (*const u8, usize) = {
            let self_trimmed: &str = self.trim();
            (self_trimmed.as_ptr(), self_trimmed.len())
        };
        unsafe {
            core::ptr::copy(
                start,
                self.as_bytes_mut().as_mut_ptr(), // no str::as_mut_ptr() in std ...
                len,
            );
        }
        self.truncate(len); // no String::set_len() in std ...
    }
}