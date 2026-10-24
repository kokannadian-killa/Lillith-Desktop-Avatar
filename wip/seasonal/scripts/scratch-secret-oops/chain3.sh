#!/bin/bash
cd /workspace/lillith/scratch-secret-oops
until grep -q CHAIN2DONE chain2.log; do sleep 15; done
python3 speckcheck.py
echo CHAIN3DONE
