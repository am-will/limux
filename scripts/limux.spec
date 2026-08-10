%global debug_package %{nil}

Name:       limux
Version:    %{version}
Release:    1%{?dist}
Summary:    GPU-accelerated terminal workspace manager for Linux
License:    MIT
URL:        https://github.com/am-will/limux
Vendor:     Will R <will@limux.dev>
ExclusiveArch: x86_64 aarch64
AutoReq:    yes
Requires:   webkitgtk6.0
Source0:    limux-%{version}.tar.gz

%description
Limux is a terminal workspace manager powered by Ghostty's GPU-rendered
terminal engine, with split panes, tabbed workspaces, and a built-in browser.

%prep
%setup -q

%build

%install
rm -rf %{buildroot}
mkdir -p %{buildroot}
cp -a %{_builddir}/limux-%{version}/usr %{buildroot}/
cp -a %{_builddir}/limux-%{version}/etc %{buildroot}/

%post
is_legacy_limux_host() {
    path="$1"
    [ -x "$path" ] || return 1
    help="$("$path" --help 2>&1 || true)"
    echo "$help" | grep -q "limux CLI" && return 1
    echo "$help" | grep -q "GApplication" && return 0
    "$path" --json identify >/tmp/limux-postinst-probe.log 2>&1 && return 1
    grep -q "Unknown option --json" /tmp/limux-postinst-probe.log
}

ldconfig 2>/dev/null || true
rm -f %{_libexecdir}/limux/limux
rm -f /usr/local/libexec/limux/limux
if is_legacy_limux_host /usr/local/bin/limux; then
    rm -f /usr/local/bin/limux
fi
rm -f %{_datadir}/applications/limux.desktop
gtk-update-icon-cache -f -t %{_datadir}/icons/hicolor 2>/dev/null || true
update-desktop-database %{_datadir}/applications 2>/dev/null || true
appstreamcli refresh-cache --force 2>/dev/null || true

%postun
ldconfig 2>/dev/null || true
gtk-update-icon-cache -f -t %{_datadir}/icons/hicolor 2>/dev/null || true
update-desktop-database %{_datadir}/applications 2>/dev/null || true
appstreamcli refresh-cache --force 2>/dev/null || true

%files
%{_bindir}/limux
%{_libexecdir}/limux/limux-host
/usr/lib/limux/libghostty-internal.so
%{_datadir}/limux/
%{_datadir}/applications/dev.limux.linux.desktop
%{_datadir}/metainfo/dev.limux.linux.metainfo.xml
%{_datadir}/icons/hicolor/
%{_sysconfdir}/ld.so.conf.d/limux.conf

%changelog
