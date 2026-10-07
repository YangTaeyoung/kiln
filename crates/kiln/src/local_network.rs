//! Explicit, best-effort local-network privacy request. No packets are sent.
//! Apple TN3179: UDP connect to randomized IPv6 link-local interface addresses.
//! Neither connect success nor failure establishes the user's permission state.
use kiln_common::{
    Theme,
    i18n::tr,
    widgets::{self, ButtonKind},
};
use std::{
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    sync::OnceLock,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Notice {
    Attempted,
    NoInterface,
    Failed,
    SettingsFailed,
}
impl Notice {
    fn text(self) -> &'static str {
        tr(match self {
            Self::Attempted => {
                "접근 요청을 시도했습니다. 허용 창이 없으면 시스템 설정 → 로컬 네트워크에서 Kiln을 확인하세요."
            }
            Self::NoInterface => {
                "접근 요청에 사용할 네트워크를 찾지 못했습니다. Wi-Fi 또는 이더넷 연결을 확인하세요."
            }
            Self::Failed => {
                "접근 요청을 시작하지 못했습니다. 시스템 설정에서 로컬 네트워크를 확인하세요."
            }
            Self::SettingsFailed => {
                "시스템 설정을 열지 못했습니다. 개인정보 보호 및 보안 → 로컬 네트워크를 직접 열어 주세요."
            }
        })
    }
}

fn supported() -> bool {
    *OnceLock::get_or_init(&SUPPORTED, || {
        objc2_foundation::NSProcessInfo::processInfo()
            .operatingSystemVersion()
            .majorVersion
            >= 15
    })
}
static SUPPORTED: OnceLock<bool> = OnceLock::new();

pub fn settings(ui: &mut egui::Ui) {
    if !supported() {
        return;
    }
    let id = ui.id().with("local-network-request-notice");
    let mut notice = ui.data(|data| data.get_temp::<Notice>(id));
    controls(ui, &mut notice, request, || {
        open::that("x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension")
    });
    if let Some(notice) = notice {
        ui.data_mut(|data| data.insert_temp(id, notice));
    }
}

fn controls(
    ui: &mut egui::Ui,
    notice: &mut Option<Notice>,
    mut request: impl FnMut() -> Notice,
    mut settings: impl FnMut() -> io::Result<()>,
) {
    widgets::group(ui, tr("개인정보 보호"), |ui| {
        widgets::setting_row(
            ui,
            tr("로컬 네트워크"),
            tr("터미널 명령과 SSH·데이터베이스의 로컬 서버 접근을 macOS에서 허용합니다."),
            |ui| {
                if widgets::icon_button(
                    ui,
                    kiln_common::icons::Icon::Gear,
                    28.0,
                    false,
                    tr("개인정보 보호 및 보안 열기"),
                )
                .clicked()
                {
                    if settings().is_err() {
                        *notice = Some(Notice::SettingsFailed);
                    }
                }
                if widgets::button(ui, tr("접근 요청"), ButtonKind::Secondary).clicked() {
                    *notice = Some(request());
                }
            },
        );
        if let Some(notice) = *notice {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(notice.text())
                        .size(12.0)
                        .color(Theme::current().text_dim),
                )
                .wrap(),
            );
        }
    });
}

fn eligible(flags: u32, address: &libc::sockaddr_in6) -> bool {
    let bytes = address.sin6_addr.s6_addr;
    flags & (libc::IFF_UP | libc::IFF_BROADCAST) as u32
        == (libc::IFF_UP | libc::IFF_BROADCAST) as u32
        && flags & (libc::IFF_LOOPBACK | libc::IFF_POINTOPOINT) as u32 == 0
        && bytes[0] == 0xfe
        && bytes[1] & 0xc0 == 0x80
}

fn targets(mut address: libc::sockaddr_in6, hosts: [[u8; 8]; 2]) -> [libc::sockaddr_in6; 2] {
    address.sin6_len = std::mem::size_of::<libc::sockaddr_in6>() as u8;
    address.sin6_port = 9_u16.to_be();
    hosts.map(|host| {
        let mut target = address;
        target.sin6_addr.s6_addr[8..].copy_from_slice(&host);
        target
    })
}

fn addresses() -> io::Result<Vec<libc::sockaddr_in6>> {
    let mut head = std::ptr::null_mut();
    // SAFETY: the OS allocates the list, freed by the guard on every return path.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Err(io::Error::last_os_error());
    }
    struct Interfaces(*mut libc::ifaddrs);
    impl Drop for Interfaces {
        fn drop(&mut self) {
            unsafe {
                libc::freeifaddrs(self.0);
            }
        }
    }
    let _list = Interfaces(head);
    let mut hosts = [[0_u8; 8]; 2];
    // SAFETY: this writable buffer contains exactly 16 bytes.
    unsafe {
        libc::arc4random_buf(hosts.as_mut_ptr().cast(), 16);
    }
    let mut result = Vec::new();
    let mut node = head;
    while !node.is_null() {
        // SAFETY: node and addresses belong to the still-live getifaddrs list;
        // check the family and provided length before copying a sockaddr_in6.
        unsafe {
            let interface = &*node;
            let sa = interface.ifa_addr;
            if !sa.is_null()
                && (*sa).sa_family as i32 == libc::AF_INET6
                && (*sa).sa_len as usize >= std::mem::size_of::<libc::sockaddr_in6>()
            {
                let mut address = sa.cast::<libc::sockaddr_in6>().read_unaligned();
                if eligible(interface.ifa_flags, &address) {
                    if address.sin6_scope_id == 0 && !interface.ifa_name.is_null() {
                        address.sin6_scope_id = libc::if_nametoindex(interface.ifa_name);
                    }
                    if address.sin6_scope_id != 0 {
                        result.extend(targets(address, hosts));
                    }
                }
            }
            node = interface.ifa_next;
        }
    }
    Ok(result)
}

fn request() -> Notice {
    let Ok(addresses) = addresses() else {
        return Notice::Failed;
    };
    attempt(&addresses, |address| {
        // SAFETY: socket returns an owned descriptor. UDP connect only establishes
        // a local peer; deliberately never use send/write/sendto on this socket.
        let descriptor = unsafe { libc::socket(libc::AF_INET6, libc::SOCK_DGRAM, 0) };
        if descriptor < 0 {
            return false;
        }
        let socket = unsafe { OwnedFd::from_raw_fd(descriptor) };
        unsafe {
            libc::fcntl(socket.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK);
            let _ = libc::connect(
                socket.as_raw_fd(),
                (address as *const libc::sockaddr_in6).cast(),
                std::mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t,
            );
        }
        true // The operation was attempted, never a permission verdict.
    })
}

fn attempt(
    addresses: &[libc::sockaddr_in6],
    mut connect: impl FnMut(&libc::sockaddr_in6) -> bool,
) -> Notice {
    if addresses.is_empty() {
        return Notice::NoInterface;
    }
    let mut attempted = false;
    for address in addresses {
        attempted |= connect(address);
    }
    if attempted {
        Notice::Attempted
    } else {
        Notice::Failed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn link_local() -> libc::sockaddr_in6 {
        let mut address: libc::sockaddr_in6 = unsafe { std::mem::zeroed() };
        address.sin6_family = libc::AF_INET6 as libc::sa_family_t;
        address.sin6_addr.s6_addr[0] = 0xfe;
        address.sin6_addr.s6_addr[1] = 0x80;
        address.sin6_scope_id = 7;
        address
    }
    #[test]
    fn local_network_candidates_exclude_loopback_vpn_down_and_global_addresses() {
        let lan = (libc::IFF_UP | libc::IFF_BROADCAST) as u32;
        assert!(eligible(lan, &link_local()));
        for flags in [
            libc::IFF_BROADCAST as u32,
            lan | libc::IFF_LOOPBACK as u32,
            lan | libc::IFF_POINTOPOINT as u32,
        ] {
            assert!(!eligible(flags, &link_local()));
        }
        let mut global = link_local();
        global.sin6_addr.s6_addr[0] = 0x20;
        assert!(!eligible(lan, &global));
        let mut multicast = link_local();
        multicast.sin6_addr.s6_addr[0] = 0xff;
        assert!(!eligible(lan, &multicast));
    }
    #[test]
    fn local_network_targets_preserve_scope_prefix_and_use_two_discard_peers() {
        let base = link_local();
        let peers = targets(base, [[1; 8], [2; 8]]);
        for peer in peers {
            assert_eq!(peer.sin6_scope_id, 7);
            assert_eq!(peer.sin6_port.to_be(), 9);
            assert_eq!(&peer.sin6_addr.s6_addr[..8], &base.sin6_addr.s6_addr[..8]);
        }
        assert_ne!(peers[0].sin6_addr.s6_addr, peers[1].sin6_addr.s6_addr);
    }
    #[test]
    fn local_network_attempt_never_reports_allowed_or_denied_and_visits_every_interface() {
        let peers = targets(link_local(), [[1; 8], [2; 8]]);
        let mut calls = 0;
        assert_eq!(
            attempt(&[], |_| panic!("no operation on empty interfaces")),
            Notice::NoInterface
        );
        assert_eq!(
            attempt(&peers, |_| {
                calls += 1;
                true
            }),
            Notice::Attempted
        );
        assert_eq!(calls, 2);
        assert_eq!(attempt(&peers, |_| false), Notice::Failed);
    }
    #[test]
    fn local_network_controls_only_request_on_click_and_keep_a_neutral_result() {
        use egui_kittest::{Harness, kittest::Queryable};
        for language in kiln_common::i18n::Language::ALL {
            kiln_common::i18n::with_language(language, || {
                for width in [360.0, 680.0] {
                    let mut h = Harness::builder().with_size([width, 420.0]).build_ui_state(
                        |ui, state: &mut (Option<Notice>, usize)| {
                            let id = egui::Id::new("local-network-test-fonts");
                            if !ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false) {
                                ui.ctx().set_fonts(kiln_common::fonts::definitions(false));
                                ui.ctx().data_mut(|d| d.insert_temp(id, true));
                                return;
                            }
                            controls(
                                ui,
                                &mut state.0,
                                || {
                                    state.1 += 1;
                                    Notice::Attempted
                                },
                                || Err(io::Error::other("fixture")),
                            );
                        },
                        (None, 0),
                    );
                    h.run_steps(3);
                    assert_eq!(h.state().1, 0);
                    h.get_by_label(tr("접근 요청")).click();
                    h.run_steps(2);
                    assert_eq!(h.state().1, 1);
                    assert_eq!(h.state().0, Some(Notice::Attempted));
                    let button = h.get_by_label(tr("접근 요청")).rect();
                    assert!(
                        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 420.0))
                            .contains_rect(button)
                    );
                    h.get_by_label(tr("개인정보 보호 및 보안 열기")).click();
                    h.run_steps(2);
                    assert_eq!(h.state().0, Some(Notice::SettingsFailed));
                }
            });
        }
    }

    #[test]
    #[ignore = "private rendered privacy fixtures; no real permission requests"]
    fn local_network_rendered_settings_captures() {
        for language in kiln_common::i18n::Language::ALL {
            kiln_common::i18n::with_language(language, || {
                for theme in ["kiln-dark", "kiln-light"] {
                    Theme::set_current(theme);
                    for width in [360.0, 680.0] {
                        for notice in [
                            None,
                            Some(Notice::Attempted),
                            Some(Notice::NoInterface),
                            Some(Notice::SettingsFailed),
                        ] {
                            let mut h = egui_kittest::Harness::builder()
                                .with_size([width, 440.0])
                                .wgpu()
                                .build_ui(move |ui| {
                                    Theme::current().apply(ui.ctx());
                                    let id = egui::Id::new("local-network-capture-fonts");
                                    if !ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false) {
                                        ui.ctx().set_fonts(kiln_common::fonts::definitions(false));
                                        ui.ctx().data_mut(|d| d.insert_temp(id, true));
                                        return;
                                    }
                                    let mut notice = notice;
                                    controls(
                                        ui,
                                        &mut notice,
                                        || panic!("fixture must not request permissions"),
                                        || panic!("fixture must not open settings"),
                                    );
                                });
                            h.ctx.set_zoom_factor(1.3);
                            h.run_steps(4);
                            h.render().unwrap().save(format!("/tmp/kiln-012-network-{language:?}-{theme}-{width}-{notice:?}.png")).unwrap();
                        }
                    }
                }
            });
        }
        Theme::set_current("kiln-dark");
    }
}
