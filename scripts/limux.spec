Name:       limux
Version:    %{version}
Release:    1%{?dist}
Summary:    GPU-accelerated terminal workspace manager for Linux
License:    MIT
URL:        https://github.com/am-will/limux
Vendor:     Will R <will@limux.dev>
BuildArch:  x86_64
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
mkdir -p %{buildroot}%{_bindir} %{buildroot}%{_libdir}/limux \
         %{buildroot}%{_datadir}/limux %{buildroot}%{_datadir}/applications \
         %{buildroot}%{_datadir}/metainfo %{buildroot}%{_datadir}/icons/hicolor/scalable/actions \
         %{buildroot}%{_sysconfdir}/ld.so.conf.d

# Binary
install -Dm755 %{_builddir}/limux-%{version}/limux %{buildroot}%{_bindir}/limux

# Shared library
install -Dm755 %{_builddir}/limux-%{version}/libghostty.so %{buildroot}%{_libdir}/limux/libghostty.so

# Ghostty resources
cp -r %{_builddir}/limux-%{version}/share/limux/ghostty %{buildroot}%{_datadir}/limux/
cp -r %{_builddir}/limux-%{version}/share/limux/terminfo %{buildroot}%{_datadir}/limux/

# Desktop file
install -Dm644 %{_builddir}/limux-%{version}/share/applications/dev.limux.linux.desktop %{buildroot}%{_datadir}/applications/dev.limux.linux.desktop

# Metainfo
install -Dm644 %{_builddir}/limux-%{version}/share/metainfo/dev.limux.linux.metainfo.xml %{buildroot}%{_datadir}/metainfo/dev.limux.linux.metainfo.xml

# Icons
cp -r %{_builddir}/limux-%{version}/share/icons/hicolor %{buildroot}%{_datadir}/icons/

# ldconfig trigger
echo "%{_libdir}/limux" > %{buildroot}%{_sysconfdir}/ld.so.conf.d/limux.conf

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
