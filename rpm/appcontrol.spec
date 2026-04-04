#
# spec file for package appcontrol
#
# Copyright (c) 2026 SUSE LLC and contributors
#
# All modifications and additions to the file contributed by third parties
# remain the property of their copyright owners, unless otherwise agreed
# upon. The license for this file, and modifications and additions to the
# file, is the same license as for the pristine package itself (unless the
# license for the pristine package is not an Open Source License, in which
# case the license is the MIT License). An "Open Source License" is a
# license that conforms to the Open Source Definition (Version 1.9)
# published by the Open Source Initiative.

# Please submit bugfixes or comments via https://bugs.opensuse.org/
#

Name:           appcontrol
Version:        0.0.1
Release:        0
Summary:        App Control utilities

License:        GPL-2.0
Source0:        %{name}-%{version}.tar.gz
Source1:        vendor.tar.gz

BuildRequires:  systemd-rpm-macros
Requires:       systemd
BuildRequires:  cargo
BuildRequires:  cargo-packaging
ExclusiveArch:  %{rust_tier1_arches}

%description
A app control application to limit the usage of
some applications, e.g. Games

%prep
%autosetup -p1 -a1

%build
export CARGO_HOME=$PWD/.cargo
%{cargo_build} --all

%install

%{cargo_install -p xpopup}
%{cargo_install -p appcontrold}

install -D -m 0644 %{_sourcedir}/appcontrold.service %{buildroot}%{_unitdir}/appcontrold.service

%check
export CARGO_HOME=$PWD/.cargo
%{cargo_test}

%post
%systemd_post appcontrold.service

%preun
%systemd_preun appcontrold.service

%postun
%systemd_postun_with_restart appcontrold.service

%files
%{_bindir}/appcontrold
%{_bindir}/xpopup
# %dir /etc/appcontrold
# %config(noreplace) /etc/appcontroll/config.toml
%{_unitdir}/appcontrold.service

%changelog
