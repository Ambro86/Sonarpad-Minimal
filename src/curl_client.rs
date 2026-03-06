use curl::easy::{Easy, List};
use std::time::Duration;

pub struct CurlClient;

impl CurlClient {
    pub fn fetch_url_impersonated(url: &str) -> Result<Vec<u8>, String> {
        Self::fetch_with_profile(url, false)
    }

    pub fn fetch_url_iphone_impersonated(url: &str) -> Result<Vec<u8>, String> {
        Self::fetch_with_profile(url, true)
    }

    pub fn post_form_impersonated(
        url: &str,
        body: &str,
        headers: &[&str],
    ) -> Result<Vec<u8>, String> {
        let mut easy = Easy::new();
        easy.url(url).map_err(|err| err.to_string())?;
        easy.follow_location(true).map_err(|err| err.to_string())?;
        easy.timeout(Duration::from_secs(30))
            .map_err(|err| err.to_string())?;
        easy.connect_timeout(Duration::from_secs(30))
            .map_err(|err| err.to_string())?;
        easy.accept_encoding("gzip, deflate, br")
            .map_err(|err| err.to_string())?;
        easy.cookie_file("").map_err(|err| err.to_string())?;
        easy.post(true).map_err(|err| err.to_string())?;
        easy.post_fields_copy(body.as_bytes())
            .map_err(|err| err.to_string())?;

        let mut list = List::new();
        for header in headers {
            list.append(header).map_err(|err| err.to_string())?;
        }
        easy.http_headers(list).map_err(|err| err.to_string())?;

        let mut data = Vec::new();
        {
            let mut transfer = easy.transfer();
            transfer
                .write_function(|new_data| {
                    data.extend_from_slice(new_data);
                    Ok(new_data.len())
                })
                .map_err(|err| err.to_string())?;
            transfer.perform().map_err(|err| err.to_string())?;
        }
        Ok(data)
    }

    fn fetch_with_profile(url: &str, iphone_profile: bool) -> Result<Vec<u8>, String> {
        let mut easy = Easy::new();
        easy.url(url).map_err(|err| err.to_string())?;
        easy.follow_location(true).map_err(|err| err.to_string())?;
        easy.max_redirections(10).map_err(|err| err.to_string())?;
        easy.timeout(Duration::from_secs(30))
            .map_err(|err| err.to_string())?;
        easy.connect_timeout(Duration::from_secs(10))
            .map_err(|err| err.to_string())?;
        easy.accept_encoding("gzip, deflate, br")
            .map_err(|err| err.to_string())?;
        easy.cookie_file("").map_err(|err| err.to_string())?;
        easy.pipewait(true).map_err(|err| err.to_string())?;

        let mut headers = List::new();
        if iphone_profile {
            headers
                .append("User-Agent: Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1")
                .map_err(|err| err.to_string())?;
            headers
                .append("Accept: text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
                .map_err(|err| err.to_string())?;
            headers
                .append("Accept-Language: it-IT,it;q=0.9,en-US;q=0.8")
                .map_err(|err| err.to_string())?;
            headers
                .append("Upgrade-Insecure-Requests: 1")
                .map_err(|err| err.to_string())?;
            headers
                .append("Connection: keep-alive")
                .map_err(|err| err.to_string())?;
        } else {
            headers
                .append("User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
                .map_err(|err| err.to_string())?;
            headers
                .append("Accept: text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8")
                .map_err(|err| err.to_string())?;
            headers
                .append("Accept-Language: it-IT,it;q=0.9,en-US;q=0.8")
                .map_err(|err| err.to_string())?;
            headers
                .append("Cache-Control: no-cache")
                .map_err(|err| err.to_string())?;
            headers
                .append("Pragma: no-cache")
                .map_err(|err| err.to_string())?;
            headers
                .append("Upgrade-Insecure-Requests: 1")
                .map_err(|err| err.to_string())?;
            headers
                .append("Connection: keep-alive")
                .map_err(|err| err.to_string())?;
        }
        easy.http_headers(headers).map_err(|err| err.to_string())?;

        let mut data = Vec::new();
        {
            let mut transfer = easy.transfer();
            transfer
                .write_function(|new_data| {
                    data.extend_from_slice(new_data);
                    Ok(new_data.len())
                })
                .map_err(|err| err.to_string())?;
            transfer.perform().map_err(|err| err.to_string())?;
        }
        Ok(data)
    }
}
