//! What the page runs on, as far as its first quality goes (issue #55):
//! a phone's or a tablet's browser starts at the project's Low, a
//! desktop's at the project's default (`auto`: the calibration).
//! Browser-free, so the host tests run it.

/// A phone or a tablet, from what `navigator` says: Client Hints'
/// `userAgentData.mobile` where the browser has them (Chromium), else the
/// user agent's `Mobi` / `Android` / `iPhone` / `iPad` / `iPod`, else an
/// iPad passing as a Mac (Safari's desktop mode: `MacIntel` with a touch
/// screen).
pub fn is_mobile(
    ua_data_mobile: Option<bool>,
    user_agent: &str,
    platform: &str,
    max_touch_points: u32,
) -> bool {
    if ua_data_mobile == Some(true) {
        return true;
    }
    let ua = user_agent.to_ascii_lowercase();
    if ["mobi", "android", "iphone", "ipad", "ipod"]
        .iter()
        .any(|k| ua.contains(k))
    {
        return true;
    }
    platform == "MacIntel" && max_touch_points > 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phones_and_tablets_are_mobile_and_desktops_are_not() {
        let pixel = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 \
                     (KHTML, like Gecko) Chrome/126.0 Mobile Safari/537.36";
        let iphone = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) \
                      AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148";
        let mac = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 \
                   (KHTML, like Gecko) Version/17.5 Safari/605.1.15";
        let windows = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                       (KHTML, like Gecko) Chrome/126.0 Safari/537.36";
        assert!(is_mobile(Some(true), windows, "Win32", 0), "client hints");
        assert!(is_mobile(None, pixel, "Linux armv8l", 5));
        assert!(is_mobile(None, iphone, "iPhone", 5));
        assert!(
            is_mobile(None, mac, "MacIntel", 5),
            "an iPad in desktop mode"
        );
        assert!(!is_mobile(None, mac, "MacIntel", 0));
        assert!(
            !is_mobile(Some(false), windows, "Win32", 10),
            "a touch laptop"
        );
        assert!(!is_mobile(None, windows, "Win32", 0));
    }
}
