# CWE catalog generator

`cwe-software-development.yaml` and `cwe-research.yaml` under
`../../templates/concerns/` are generated from MITRE's published CWE XML.
To refresh:

```bash
# 1. Download the latest CWE XML release from MITRE:
#    https://cwe.mitre.org/data/downloads.html
curl -L -o build/cwe/cwec_v4.13.xml.zip \
  https://cwe.mitre.org/data/xml/cwec_v4.13.xml.zip
unzip -o build/cwe/cwec_v4.13.xml.zip -d build/cwe/

# 2. Run the generator for each view (run from the crate root):
cargo run --features gen --bin gen_cwe_catalog -- \
  --xml build/cwe/cwec_v4.13.xml \
  --view 699 \
  --release 4.13 \
  --out templates/concerns/cwe-software-development.yaml

cargo run --features gen --bin gen_cwe_catalog -- \
  --xml build/cwe/cwec_v4.13.xml \
  --view 1000 \
  --release 4.13 \
  --out templates/concerns/cwe-research.yaml
```

The XML/zip files are gitignored (large, re-downloadable). The generated
YAML files and their `.version.txt` sidecars ARE committed.
