use std::str::FromStr;

use std::io::BufRead;

use async_std::fs::OpenOptions;
use async_std::io::BufWriter;
use async_std::prelude::*;

use crate::{doh, Domain};

const DNS_RECORD_DIR: &'static str = "dns_db";
const MAX_AGE: u64 = 7 * 86400;

#[derive(Clone, Debug)]
pub struct DNSResultRecord {
    pub domain: Domain,
    pub cnames: Vec<Domain>,
    pub ips: Vec<std::net::IpAddr>,
}

impl DNSResultRecord {
    fn to_string(&self) -> String {
        let mut output = String::new();
        output.push_str(&self.domain);
        output.push(';');
        for cname in self.cnames.iter() {
            output.push_str(&cname);
            output.push(',');
        }
        output.push(';');
        for ip in self.ips.iter() {
            output.push_str(&ip.to_string());
            output.push(',');
        }
        output
    }
}

impl FromStr for DNSResultRecord {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split(';');
        let domain: Domain = parts.next().ok_or(())?.parse().map_err(|_| ())?;
        let mut cnames: Vec<Domain> = Vec::new();
        for cname in parts.next().ok_or(())?.split(',').filter(|c| !c.is_empty()) {
            cnames.push(cname.parse().map_err(|_| ())?)
        }
        let mut ips: Vec<std::net::IpAddr> = Vec::new();
        for ip in parts
            .next()
            .ok_or(())?
            .trim_end()
            .split(',')
            .filter(|c| !c.is_empty())
        {
            ips.push(ip.parse().map_err(|_| ())?)
        }
        Ok(DNSResultRecord {
            domain,
            cnames,
            ips,
        })
    }
}

async fn get_dns_results(
    client: &reqwest::Client,
    server: &str,
    domain: Domain,
) -> Result<DNSResultRecord, Box<dyn std::error::Error>> {
    Ok(doh::lookup_domain(&server, &client, 3, &domain)
        .await?
        .unwrap_or_else(|| DNSResultRecord {
            domain: domain,
            cnames: Vec::new(),
            ips: Vec::new(),
        }))
}

pub async fn lookup_domains<F>(
    mut domains: std::collections::HashSet<Domain>,
    mut f: F,

    servers: &[String],
    client: &reqwest::Client,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnMut(&Domain, &[Domain], &[std::net::IpAddr]) -> (),
{
    let _ = std::fs::create_dir(DNS_RECORD_DIR);
    for entry in std::fs::read_dir(DNS_RECORD_DIR)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if let Ok(modified) = metadata.modified().or(metadata.created()) {
            let now = std::time::SystemTime::now();
            if let Ok(duration_since) = now.duration_since(modified) {
                if duration_since.as_secs() < MAX_AGE {
                    if let Ok(file) = std::fs::File::open(entry.path()) {
                        let mut file = std::io::BufReader::new(file);
                        let mut line = String::new();
                        while let Ok(len) = file.read_line(&mut line) {
                            if len == 0 {
                                break;
                            }
                            if let Ok(record) = line.parse::<DNSResultRecord>() {
                                domains.remove(&record.domain);
                                f(&record.domain, &record.cnames, &record.ips)
                            }
                            line.clear();
                        }
                    }

                    continue;
                }
            }
        }
        println!("Removing expired record");
    }

    println!("Looking up {} domains", domains.len());
    if domains.is_empty() {
        return Ok(());
    }

    let mut path = std::path::PathBuf::from(DNS_RECORD_DIR);
    path.push(std::path::PathBuf::from(format!(
        "{:?}",
        chrono::Utc::today()
    )));
    let mut wtr = BufWriter::new(
        OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .await?,
    );
    wtr.write_all(b"\n").await?;
    let total_length = domains.len();
    let mut domain_iter = domains.into_iter();
    let mut tasks = futures::stream::FuturesUnordered::new();
    for (i, domain) in (0..500).zip(&mut domain_iter) {
        tasks.push(get_dns_results(
            &client,
            &servers[i % servers.len()],
            domain,
        ));
    }
    let now = std::time::Instant::now();
    let mut i = 0;
    let mut error_count = 0;
    while let Some(record) = tasks.next().await {
        if let Ok(record) = record {
            if i % 100 == 0 {
                println!(
                    "{}/{} {}/s with {} errors: Got response for {}",
                    i,
                    total_length,
                    i as f32 / now.elapsed().as_secs_f32(),
                    error_count,
                    &record.domain
                );
            }
            f(&record.domain, &record.cnames, &record.ips);
            wtr.write_all(record.to_string().as_bytes()).await?;
            wtr.write_all(b"\n").await?;
        } else {
            error_count += 1;
        }
        if let Some(next_domain) = domain_iter.next() {
            tasks.push(get_dns_results(
                &client,
                &servers[i % servers.len()],
                next_domain,
            ));
            i += 1;
        }
    }
    wtr.flush().await?;
    Ok(())
}