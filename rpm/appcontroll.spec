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
Url:            https://github.com/cfconrad/appcontrol.git
Source0:        %{name}-%{version}.tar.zst
Source1:        registry.tar.zst


# Require systemd for macros
BuildRequires:  systemd-rpm-macros
Requires:       systemd

# Pull in the latest rust/cargo toolchain
BuildRequires:  cargo
# This contains rpm macros to assist with building
BuildRequires:  cargo-packaging
# Disable this line if you wish to support all platforms.
# In most situations, you will likely only target tier1 arches for user facing components.
ExclusiveArch:  %{rust_tier1_arches}

%description
A app control application to limit the usage of
some applications, e.g. Games

%prep
# The number passed to -a (a stands for "after") should be equivalent to the Source tag number
# of the vendor tarball, 1 in this case (from Source1).
%autosetup -p1 -a1
# Remove exec bits to prevent an issue in fedora shebang checking. Uncomment only if required.
# find vendor -type f -name \*.rs -exec chmod -x '{}' \;


%build
export CARGO_HOME=$PWD/.cargo
%{cargo_build} --all

%install

%{cargo_install -p project1}
%{cargo_install -p project2}

# Install systemd service
install -D -m 0644 appcontrold.service %{buildroot}/usr/lib/systemd/system/appcontrold.service

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
/usr/bin/appcontrold
%dir /etc/appcontrold
#%config(noreplace) /etc/appcontrol/config.toml
/usr/lib/systemd/system/appcontrold.service

%changelog
