Name:       limux
Version:    %{version}
Release:    1%{?dist}
Summary:    GPU-accelerated terminal workspace manager for Linux
License:    MIT
URL:        https://github.com/am-will/limux
Vendor:     Will R <will@limux.dev>
ExclusiveArch: x86_64 aarch64
AutoReq:    yes
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
ldconfig 2>/dev/null || true
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
%{_libdir}/limux/libghostty.so
%{_datadir}/limux/
%{_datadir}/applications/dev.limux.linux.desktop
%{_datadir}/metainfo/dev.limux.linux.metainfo.xml
%{_datadir}/icons/hicolor/
%{_sysconfdir}/ld.so.conf.d/limux.conf

%changelog
