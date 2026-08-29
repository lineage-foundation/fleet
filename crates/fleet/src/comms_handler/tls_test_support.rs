//! TLS test-support helpers for `comms_handler`.
//!
//! These are used directly by `comms_handler`'s own `#[cfg(test)]` code, and
//! are re-exported from `crate::test_utils` so existing node test callers
//! keep working unchanged.

use crate::comms_handler::{test_tls_certificates, TcpTlsConfig, TcpTlsListner};
use crate::configurations::TlsSpec;
use crate::utils::concat_maps;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use tracing::info;

#[derive(Clone, Default)]
pub struct TestTlsSpec {
    pub pem_certificates: BTreeMap<String, String>,
    pub pem_pkcs8_private_keys: BTreeMap<String, String>,
    pub pem_certificates_with_ca: BTreeMap<String, String>,
    pub pem_pkcs8_private_keys_with_ca: BTreeMap<String, String>,
}

impl TestTlsSpec {
    pub fn make_tls_spec(&self, socket_name_mapping: &BTreeMap<SocketAddr, String>) -> TlsSpec {
        TlsSpec {
            socket_name_mapping: socket_name_mapping.clone(),
            pem_certificates: concat_maps(&self.pem_certificates, &self.pem_certificates_with_ca),
            pem_pkcs8_private_keys: concat_maps(
                &self.pem_pkcs8_private_keys,
                &self.pem_pkcs8_private_keys_with_ca,
            ),
            untrusted_names: Some(self.pem_certificates_with_ca.keys().cloned().collect()),
            pem_certificate_override: None,
            pem_pkcs8_private_key_override: None,
        }
    }
}

pub fn get_test_tls_name(name: &str, spec: &TestTlsSpec) -> String {
    let tls_name = format!("{name}.lineage.foundation");
    if spec.pem_certificates.contains_key(&tls_name)
        || spec.pem_certificates_with_ca.contains_key(&tls_name)
    {
        tls_name
    } else {
        "node.lineage.foundation".to_owned()
    }
}

pub fn get_test_tls_spec() -> TestTlsSpec {
    TestTlsSpec {
        pem_certificates: test_tls_certificates::TEST_PEM_CERTIFICATES
            .iter()
            .copied()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect(),
        pem_pkcs8_private_keys: test_tls_certificates::TEST_PKCS8_KEYS
            .iter()
            .copied()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect(),
        pem_certificates_with_ca: test_tls_certificates::TEST_PEM_CERTIFICATES_WITH_CA
            .iter()
            .copied()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect(),
        pem_pkcs8_private_keys_with_ca: test_tls_certificates::TEST_PKCS8_KEYS_WITH_CA
            .iter()
            .copied()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect(),
    }
}

pub fn get_common_tls_config() -> TcpTlsConfig {
    let addr = "127.0.0.1:0".parse().unwrap();
    let mapping = vec![(addr, "node.lineage.foundation".to_owned())]
        .into_iter()
        .collect();
    let tls_spec = get_test_tls_spec().make_tls_spec(&mapping);
    TcpTlsConfig::from_tls_spec(addr, &tls_spec).unwrap()
}

pub async fn get_bound_common_tls_configs(
    names: &[&str],
    update_spec: impl Fn(&str, TlsSpec) -> TlsSpec,
) -> Vec<TcpTlsConfig> {
    let mut mapping = BTreeMap::new();
    let mut listeners = Vec::new();
    let tls_spec = get_test_tls_spec();
    for name in names.iter().copied() {
        let mut address = "127.0.0.1:0".parse().unwrap();
        let tcp_listener = TcpTlsListner::new_raw_listner(address).await.unwrap();
        address.set_port(tcp_listener.local_addr().unwrap().port());

        info!("Bound name address: {}: {:?}", name, address);
        mapping.insert(address, get_test_tls_name(name, &tls_spec));
        listeners.push((address, tcp_listener));
    }

    let mut configs = Vec::new();
    for (address, tcp_listener) in listeners.drain(..) {
        let name = &mapping[&address];
        let tls_spec = update_spec(name, tls_spec.make_tls_spec(&mapping));
        let config = TcpTlsConfig::from_tls_spec(address, &tls_spec).unwrap();
        let config = config.with_listener(tcp_listener).await;
        configs.push(config);
    }
    configs
}
