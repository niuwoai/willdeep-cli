# 压缩配置服务验收

通过 19，失败 0。

| 用例 | 结果 |
|---|---|
| plan previews all three compressor fields without writing | passed |
| apply requires confirmation | passed |
| apply saves three fields with an exact backup | passed |
| invalid compressor window 3999 is refused | passed |
| invalid compressor window 1048577 is refused | passed |
| invalid compressor window -1 is refused | passed |
| invalid compressor window 0 is refused | passed |
| invalid compressor window 1.5 is refused | passed |
| invalid compressor window "32768" is refused | passed |
| invalid compressor profile "" is refused | passed |
| invalid compressor profile "  " is refused | passed |
| invalid compressor profile "missing" is refused | passed |
| deleting a referenced provider is refused | passed |
| creating and referencing a provider in the same batch is valid | passed |
| clearing the reference and removing its provider is valid | passed |
| concurrent saves with the same revision have one winner | passed |
| same-basename targets share a backup lock without overwriting originals | passed |
| private stdio supports a Unicode config path under the C locale | passed |
| validate catches compressor dependency and range errors | passed |
