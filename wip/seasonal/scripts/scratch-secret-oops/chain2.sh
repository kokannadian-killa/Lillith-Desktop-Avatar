#!/bin/bash
cd /workspace/lillith/scratch-secret-oops
python3 holefill.py fill /workspace/lillith/wt-secret-oops/assets/generated/secret_oops_retie/secret_oops_retie_sheet.png
./fixloop.sh secret_oops_retie
python3 holefill.py fill /workspace/lillith/wt-secret-oops/assets/generated/secret_oops_retie/secret_oops_retie_sheet.png
python3 fixall.py secret_oops_retie
./qa.sh secret_oops_retie
echo CHAIN2DONE
