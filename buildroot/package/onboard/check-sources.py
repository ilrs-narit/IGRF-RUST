#!/usr/bin/env python3
"""Check pinned archives and Onboard dconf keys: check-sources.py DOWNLOAD_DIR."""
import configparser
import hashlib
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import xml.etree.ElementTree as ET

packages = Path(__file__).resolve().parent.parent
archives = Path(sys.argv[1])
for package in ('onboard', 'dconf', 'hunspell', 'iso-codes', 'python-distutils-extra'):
    checks = (packages / package / (package + '.hash')).read_text().splitlines()
    source_hash, source_name = checks[1].split()[1:]
    source = archives / source_name
    assert hashlib.sha256(source.read_bytes()).hexdigest() == source_hash, source
    with tarfile.open(source) as archive:
        members = archive.getnames()
        for check in checks[2:]:
            _, expected, name = check.split()
            member = next(m for m in members if m.count('/') == 1 and m.endswith('/' + name))
            assert hashlib.sha256(archive.extractfile(member).read()).hexdigest() == expected, name
        if package == 'onboard':
            schema = ET.fromstring(archive.extractfile('onboard-1.4.4-1/data/org.onboard.gschema.xml').read())
        if package == 'iso-codes':
            # Run the same upstream XML generator used by the recipe.
            with tempfile.TemporaryDirectory() as directory:
                archive.extractall(directory, filter='data')
                root = Path(directory) / 'iso-codes-4.18.0'
                for domain in ('iso_639-2', 'iso_3166-1'):
                    output = root / ('iso_' + domain + '.xml')
                    subprocess.run([sys.executable, str(root / 'bin/xml_from_json.py'), domain,
                                    str(root / 'data'), str(output)], check=True)
                    assert len(ET.parse(output).getroot()) > 100
keys = {s.attrib['path'].strip('/'): {k.attrib['name'] for k in s.findall('key')}
        for s in schema.findall('schema')}
defaults = configparser.ConfigParser()
defaults.read(packages.parent / 'board/igrf/raspberrypi5/rootfs-overlay/etc/dconf/db/local.d/00-onboard')
for section in defaults.sections():
    assert set(defaults[section]) <= keys[section], section
print('All five source/license hashes, ISO XML generation, and Onboard default keys passed.')
