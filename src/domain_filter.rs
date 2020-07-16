use crate::Domain;

use std::collections::HashSet;

#[derive(Default)]
pub struct DomainFilterBuilder {
    allow_domains: HashSet<Domain>,
    disallow_domains: HashSet<Domain>,
    allow_subdomains: HashSet<Domain>,
    disallow_subdomains: HashSet<Domain>,
    allow_ips: HashSet<std::net::IpAddr>,
    disallow_ips: HashSet<std::net::IpAddr>,
    allow_ip_net: HashSet<ipnet::IpNet>,
    disallow_ip_net: HashSet<ipnet::IpNet>,
    adblock: HashSet<String>,
    allow_regex: HashSet<String>,
    disallow_regex: HashSet<String>,
}

impl DomainFilterBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_allow_domain(&mut self, domain: Domain) {
        let _ = self.disallow_domains.remove(&domain);
        self.allow_domains.insert(domain);
    }
    pub fn add_disallow_domain(&mut self, domain: Domain) {
        if !self.allow_domains.contains(&domain)
            && !is_subdomain_of_list(&domain, &self.allow_subdomains)
        {
            self.disallow_domains.insert(domain);
        }
    }
    pub fn add_allow_subdomain(&mut self, domain: Domain) {
        let _ = self.disallow_subdomains.remove(&domain);
        self.allow_subdomains.insert(domain);
    }
    pub fn add_disallow_subdomain(&mut self, domain: Domain) {
        if !self.allow_subdomains.contains(&domain) {
            self.disallow_subdomains.insert(domain);
        }
    }

    pub fn add_allow_ip_addr(&mut self, ip: std::net::IpAddr) {
        let _ = self.disallow_ips.remove(&ip);
        self.allow_ips.insert(ip);
    }
    pub fn add_disallow_ip_addr(&mut self, ip: std::net::IpAddr) {
        self.disallow_ips.insert(ip);
    }

    pub fn add_allow_ip_subnet(&mut self, net: ipnet::IpNet) {
        let _ = self.disallow_ip_net.remove(&net);
        self.allow_ip_net.insert(net);
    }

    pub fn add_disallow_ip_subnet(&mut self, ip: ipnet::IpNet) {
        self.disallow_ip_net.insert(ip);
    }

    pub fn add_adblock_rule(&mut self, rule: &str) {
        self.adblock.insert(rule.to_string());
    }

    pub fn add_allow_regex(&mut self, re: &str) {
        if regex::Regex::new(re).is_ok() {
            self.allow_regex.insert(re.to_string());
        }
    }
    pub fn add_disallow_regex(&mut self, re: &str) {
        if regex::Regex::new(re).is_ok() {
            self.disallow_regex.insert(re.to_string());
        }
    }

    pub fn to_domain_filter(&self) -> DomainFilter {
        DomainFilter {
            allow_domains: self.allow_domains.clone(),
            disallow_domains: self.disallow_domains.clone(),
            allow_subdomains: self.allow_subdomains.clone(),
            disallow_subdomains: self.disallow_subdomains.clone(),
            allow_ips: self.allow_ips.clone(),
            disallow_ips: self.disallow_ips.clone(),
            allow_ip_net: self.allow_ip_net.iter().cloned().collect(),
            disallow_ip_net: self.disallow_ip_net.iter().cloned().collect(),
            adblock: adblock::engine::Engine::from_rules(
                &self.adblock.iter().cloned().collect::<Vec<String>>(),
            ),
            allow_regex: regex::RegexSet::new(&self.allow_regex).unwrap(),
            disallow_regex: regex::RegexSet::new(&self.disallow_regex).unwrap(),
        }
    }
}

fn is_subdomain_of_list(domain: &Domain, filter_list: &std::collections::HashSet<Domain>) -> bool {
    domain
        .iter_parent_domains()
        .any(|part| filter_list.contains(&part))
}

#[allow(dead_code)]
pub struct DomainFilter {
    allow_domains: HashSet<Domain>,
    disallow_domains: HashSet<Domain>,
    allow_subdomains: HashSet<Domain>,
    disallow_subdomains: HashSet<Domain>,
    allow_ips: HashSet<std::net::IpAddr>,
    disallow_ips: HashSet<std::net::IpAddr>,
    allow_ip_net: Vec<ipnet::IpNet>,
    disallow_ip_net: Vec<ipnet::IpNet>,
    adblock: adblock::engine::Engine,
    allow_regex: regex::RegexSet,
    disallow_regex: regex::RegexSet,
}
#[allow(dead_code)]
impl DomainFilter {
    pub fn allowed(
        &self,
        domain: &Domain,
        cnames: &[Domain],
        ips: &[std::net::IpAddr],
    ) -> Option<bool> {
        if let Some(result) = self.domain_is_allowed(domain) {
            Some(result)
        } else if cnames
            .iter()
            .any(|cname| self.domain_is_allowed(cname) == Some(false))
        {
            Some(false)
        } else if ips.iter().any(|ip| self.ip_is_allowed(ip) == Some(false)) {
            Some(false)
        } else {
            None
        }
    }

    fn domain_is_allowed(&self, domain: &Domain) -> Option<bool> {
        if self.allow_domains.contains(domain)
            || is_subdomain_of_list(&*domain, &self.allow_subdomains)
            || self.allow_regex.is_match(domain)
        {
            return Some(true);
        }
        let url = format!("https://{}", domain);
        let blocker_result = self.adblock.check_network_urls(&url, &url, "");
        if blocker_result.exception.is_some() {
            // Adblock exception rule
            Some(true)
        } else if blocker_result.matched
            || self.disallow_domains.contains(domain)
            || is_subdomain_of_list(&*domain, &self.disallow_subdomains)
            || self.disallow_regex.is_match(domain)
        {
            Some(false)
        } else {
            None
        }
    }

    fn ip_is_allowed(&self, ip: &std::net::IpAddr) -> Option<bool> {
        if self.allow_ips.contains(ip) || self.allow_ip_net.iter().any(|net| net.contains(ip)) {
            Some(true)
        } else if self.disallow_ips.contains(ip)
            || self.disallow_ip_net.iter().any(|net| net.contains(ip))
        {
            Some(false)
        } else {
            None
        }
    }
}

#[test]
fn default_unblocked() {
    assert_eq!(
        DomainFilterBuilder::new()
            .to_domain_filter()
            .domain_is_allowed(&"example.org".parse().unwrap()),
        None
    )
}

#[test]
fn regex_disallow_all_blocks_domain() {
    let mut filter = DomainFilterBuilder::new();
    filter.add_disallow_regex(".");
    let filter = filter.to_domain_filter();
    assert_eq!(
        filter.domain_is_allowed(&"example.org".parse().unwrap()),
        Some(false)
    )
}
#[test]
fn regex_allow_overrules_regex_disallow() {
    let mut filter = DomainFilterBuilder::new();
    filter.add_disallow_regex(".");
    filter.add_allow_regex(".");
    let filter = filter.to_domain_filter();
    assert_eq!(
        filter.domain_is_allowed(&"example.org".parse().unwrap()),
        Some(true)
    )
}

#[test]
fn adblock_can_block_domain() {
    let mut filter = DomainFilterBuilder::new();
    filter.add_adblock_rule("||example.com^");
    let filter = filter.to_domain_filter();
    assert_eq!(
        filter.domain_is_allowed(&"example.com".parse().unwrap()),
        Some(false)
    )
}

#[test]
fn adblock_can_whitelist_blocked_domain() {
    let mut filter = DomainFilterBuilder::new();
    filter.add_disallow_regex(".");
    // Due to the adblock rule optimiser,
    // exception rules which don't overlap with block rules are ignored
    filter.add_adblock_rule("||example.com^");
    filter.add_adblock_rule("@@||example.com^");
    let filter = filter.to_domain_filter();
    assert_eq!(
        filter.domain_is_allowed(&"example.com".parse().unwrap()),
        Some(true)
    )
}

#[test]
fn subdomain_disallow_blocks() {
    let mut filter = DomainFilterBuilder::new();
    filter.add_disallow_subdomain("example.com".parse().unwrap());
    let filter = filter.to_domain_filter();
    assert_eq!(
        filter.domain_is_allowed(&"www.example.com".parse().unwrap()),
        Some(false)
    )
}

#[test]
fn subdomain_allow_whitelists_domains() {
    let mut filter = DomainFilterBuilder::new();
    filter.add_disallow_regex(".");
    filter.add_allow_subdomain("example.com".parse().unwrap());
    let filter = filter.to_domain_filter();
    assert_eq!(
        filter.domain_is_allowed(&"www.example.com".parse().unwrap()),
        Some(true)
    )
}

#[test]
fn subdomain_disallow_does_not_block_domain() {
    let mut filter = DomainFilterBuilder::new();
    filter.add_disallow_subdomain("example.com".parse().unwrap());
    let filter = filter.to_domain_filter();
    assert_eq!(
        filter.domain_is_allowed(&"example.com".parse().unwrap()),
        None
    )
}

#[test]
fn blocked_cname_blocks_base() {
    let mut filter = DomainFilterBuilder::new();
    filter.add_disallow_domain("tracker.com".parse().unwrap());
    let filter = filter.to_domain_filter();
    assert_eq!(
        filter.allowed(
            &"example.com".parse().unwrap(),
            &["tracker.com".parse().unwrap()],
            &[]
        ),
        Some(false)
    )
}

#[test]
fn blocked_ip_blocks_base() {
    let mut filter = DomainFilterBuilder::new();
    filter.add_disallow_ip_addr("8.8.8.8".parse().unwrap());
    let filter = filter.to_domain_filter();
    assert_eq!(
        filter.allowed(
            &"example.com".parse().unwrap(),
            &[],
            &["8.8.8.8".parse().unwrap()]
        ),
        Some(false)
    )
}

#[test]
fn ignores_allowed_ips() {
    let mut filter = DomainFilterBuilder::new();
    filter.add_disallow_domain("example.com".parse().unwrap());
    filter.add_allow_ip_addr("8.8.8.8".parse().unwrap());
    let filter = filter.to_domain_filter();
    assert_eq!(
        filter.allowed(
            &"example.com".parse().unwrap(),
            &[],
            &["8.8.8.8".parse().unwrap()]
        ),
        Some(false)
    )
}

#[test]
fn unblocked_ips_do_not_allow() {
    let mut filter = DomainFilterBuilder::new();
    filter.add_allow_ip_addr("8.8.8.8".parse().unwrap());
    let filter = filter.to_domain_filter();
    assert_eq!(
        filter.allowed(
            &"example.com".parse().unwrap(),
            &[],
            &["8.8.8.8".parse().unwrap()]
        ),
        None
    )
}
