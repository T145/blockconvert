use crate::list_downloader::FilterListHandler;
use clap::Parser;
use domain_list_builder::*;
use futures::FutureExt;
use rand::prelude::*;
use std::sync::Arc;

const LIST_CSV: &str = "filterlists.csv";

/// Blockconvert
#[derive(Parser)]
#[clap(version = "0.1")]
struct Opts {
    #[clap(subcommand)]
    mode: Mode,
    #[clap(short, long, default_value = "config.toml")]
    config: std::path::PathBuf,
    #[clap(short, long, default_value = "db")]
    database: std::path::PathBuf,
}

#[derive(Parser)]
enum Mode {
    Generate,
    Query(Query),
    FindDomains(FindDomains),
}
#[derive(Parser)]
struct Query {
    query: String,
    #[clap(short, long)]
    ignore_dns: bool,
}
#[derive(Parser)]
struct FindDomains {
    #[clap(short, long, default_value = "64")]
    concurrent_requests: std::num::NonZeroUsize,
}

const INTERNAL_LISTS: &[(&str, FilterListType)] = &[
    ("blocklist.txt", FilterListType::DomainBlocklist),
    ("block_ips.txt", FilterListType::IPBlocklist),
    ("block_ipnets.txt", FilterListType::IPNetBlocklist),
    ("block_regex.txt", FilterListType::RegexBlocklist),
    ("adblock.txt", FilterListType::Adblock),
    ("allowlist.txt", FilterListType::DomainAllowlist),
    ("allow_regex.txt", FilterListType::RegexAllowlist),
];

fn get_internal_lists() -> Vec<(std::path::PathBuf, FilterListRecord)> {
    let mut internal = Vec::new();
    for (file_path, list_type) in INTERNAL_LISTS.iter() {
        let mut path = std::path::PathBuf::from("internal");
        path.push(file_path);
        let record = FilterListRecord {
            name: file_path.to_string(),
            url: file_path.to_string(),
            author: Default::default(),
            license: Default::default(),
            expires: Default::default(),
            list_type: *list_type,
        };
        internal.push((path, record));
    }
    internal
}

fn read_csv() -> Result<Vec<FilterListRecord>, csv::Error> {
    let path = std::path::Path::new(LIST_CSV);
    let mut records: Vec<FilterListRecord> = csv::Reader::from_path(path)?
        .deserialize()
        .map(|result| {
            let record: FilterListRecord = result?;
            Ok(record)
        })
        .filter_map(|result: Result<FilterListRecord, csv::Error>| result.ok())
        .collect();

    records.sort();
    records.reverse();
    records.dedup();
    let mut wrt = csv::Writer::from_path(path)?;
    for record in records.iter() {
        let _ = wrt.serialize(record);
    }
    let _ = wrt.flush();
    Ok(records)
}

async fn generate(mut config: config::Config) -> Result<(), anyhow::Error> {
    let client = reqwest::Client::new();
    if let Ok(records) = read_csv() {
        println!("Read CSV");
        let builder = Arc::new(FilterListBuilder::new(config.clone()));
        println!("Initialised FilterListBuilder");

        list_downloader::download_all(
            config.clone(),
            client,
            records,
            get_internal_lists(),
            builder.clone(),
        )
        .await?;

        println!("Downloaded Lists");

        let builder = Arc::try_unwrap(builder).ok().expect("Failed to unwrap Arc");
        let bc = Arc::new(builder.to_filterlist());

        db::dir_db_read(
            bc.clone(),
            &std::path::Path::new(&config.get_paths().extracted),
            config.get_max_extracted_age(),
        )
        .await?;

        bc.finished_extracting();
        let mut bc = Arc::try_unwrap(bc).ok().expect("Failed to unwrap Arc");
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::ACCEPT,
            "application/dns-json".parse().unwrap(),
        );
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .unwrap();
        println!("Checking DNS");
        let now = std::time::Instant::now();
        bc.check_dns(&client).await;
        println!("Checked DNS in {}s", now.elapsed().as_secs_f32());
        println!("Writing to file");
        let now = std::time::Instant::now();
        bc.write_all().await?;
        println!("Wrote to file in {}s", now.elapsed().as_secs_f32());
    }
    Ok(())
}

#[derive(Clone)]
struct QueryFilterListHandler {
    config: config::Config,
    parts: Vec<(Domain, Vec<Domain>, Vec<std::net::IpAddr>)>,
}

impl FilterListHandler for QueryFilterListHandler {
    fn handle_filter_list(&self, record: FilterListRecord, data: &str) {
        let bc = FilterList::from(self.config.clone(), &[(record.list_type, &data)]);
        for (part, cnames, ips) in self.parts.iter() {
            if let Some(allowed) = bc.allowed(&part, &cnames, &ips) {
                if allowed {
                    println!("ALLOW: {} allowed {}", record.url, part)
                } else {
                    println!("BLOCK: {} blocked {}", record.url, part)
                }
            }
        }
    }
}

async fn query(mut config: config::Config, q: Query) -> Result<(), anyhow::Error> {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::ACCEPT,
        "application/dns-json".parse().unwrap(),
    );
    let client = reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap();
    let domain = q.query.parse::<Domain>()?;
    let mut parts: Vec<(Domain, Vec<Domain>, Vec<std::net::IpAddr>)> = Vec::new();
    for part in std::iter::once(domain.clone()).chain(domain.iter_parent_domains()) {
        let (cnames, ips): (Vec<Domain>, Vec<std::net::IpAddr>) = if !q.ignore_dns {
            if let Some(result) = doh::lookup_domain(
                config
                    .get_dns_servers()
                    .choose(&mut rand::thread_rng())
                    .unwrap()
                    .clone(),
                client.clone(),
                3_usize,
                config.get_timeout(),
                &part,
            )
            .await?
            {
                println!("Domain: {:?}", part);
                println!("CNames: {:?}", result.cnames);
                println!("IPs: {:?}", result.ips);
                (result.cnames, result.ips)
            } else {
                Default::default()
            }
        } else {
            Default::default()
        };
        parts.push((part, cnames, ips));
    }
    let query_handler = Arc::new(QueryFilterListHandler {
        config: config.clone(),
        parts,
    });
    let client = reqwest::Client::new();
    let records = read_csv()?;

    list_downloader::download_all(
        config,
        client,
        records,
        get_internal_lists(),
        query_handler.clone(),
    )
    .await?;
    Ok(())
}

async fn find_domains(find_opts: FindDomains, db: sled::Db) -> Result<(), anyhow::Error> {
    println!("Started finding domains");
    let (tx, rx) = std::sync::mpsc::channel::<Domain>();
    let db_clone = db.clone();
    let current_lookups = Arc::new(dashmap::DashSet::<Domain>::new());
    let current_lookups_clone = current_lookups.clone();

    let (resolve_tx, mut resolve_rx) = tokio::sync::mpsc::unbounded_channel::<Domain>();
    std::thread::spawn(move || {
        let current_lookups = current_lookups_clone;
        while let Ok(domain) = rx.recv() {
            if !current_lookups.contains(&domain) {
                current_lookups.insert(domain.clone());

                let old = db_clone.get(domain.as_str());
                if old == Ok(None) {
                    if resolve_tx.send(domain).is_err() {
                        break;
                    }
                }
            }
        }
    });
    let dns_task = tokio::task::spawn(async move {
        while let Some(domain) = resolve_rx.recv().await {
            println!("Domain: {}", domain);
            current_lookups.remove(&domain);
        }
    });
    futures::select!(
        _ = tokio::task::spawn(certstream::certstream(tx)).fuse() => (),
        _ = tokio::task::spawn(async {
            let _ = tokio::signal::ctrl_c().await;
            println!("Recieved Ctrl-C");
        }).fuse() => (),
        _ = dns_task.fuse() => ()
    );
    db.flush_async().await?;

    println!("Finished finding domains");
    println!("Disk size: {:?}", db.size_on_disk());
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let opts: Opts = Opts::parse();
    let db = sled::Config::default()
        .mode(sled::Mode::HighThroughput)
        .path(opts.database)
        .open()?;

    let result = match opts.mode {
        Mode::Generate => generate(config::Config::open(opts.config.clone())?).await,
        Mode::Query(q) => query(config::Config::open(opts.config.clone())?, q).await,
        Mode::FindDomains(find_opts) => find_domains(find_opts, db).await,
    };
    if let Err(error) = &result {
        println!("Failed with error: {:?}", error);
    }
    result
}
