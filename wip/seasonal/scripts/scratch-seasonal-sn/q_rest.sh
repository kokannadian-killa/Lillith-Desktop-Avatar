#!/bin/bash
S=/workspace/lillith/scratch-seasonal-sn; V=/workspace/lillith/assets/source/video
Z="--stop 385 --zmax 2.3 --zoom 14,40,58,84,104,130,148,174 --snap --facing front"
while pgrep -f q_summer1.sh >/dev/null; do sleep 30; done
$S/zt.sh summer_popsicle_lick $V/summer_popsicle_lick/summer_popsicle_lick_chain.mp4 $Z --focus-src 650,206
$S/zt.sh summer_lotion_squeeze $V/summer_lotion_squeeze/summer_lotion_squeeze_chain.mp4 $Z --focus-src 650,206
$S/zt.sh newyear_champagne_rim_lick $V/newyear_champagne_rim_lick/newyear_champagne_rim_lick_chain.mp4 $Z --focus-src 644,200
$S/zt.sh newyear_strawberry_bite $V/newyear_strawberry_bite/newyear_strawberry_bite_chain.mp4 $Z --focus-src 638,200
$S/zt.sh newyear_midnight_kiss_squeeze $V/newyear_midnight_kiss_squeeze/newyear_midnight_kiss_squeeze_chain.mp4 $Z --focus-src 644,219
$S/zt.sh newyear_strap_oops $V/newyear_strap_oops/newyear_strap_oops_chain.mp4 $Z --focus-src 644,194
