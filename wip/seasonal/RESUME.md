# Seasonal outfits — resume notes (2026-10-24 15:35 MDT)

## Shipped (ready for the lead)
| branch | SHA | clips |
| --- | --- | --- |
| feat/seasonal-outfits | c339060 | halloween_hat_tip_wink, halloween_broom_sit_lollipop, halloween_pumpkin_hug_candy, halloween_cape_swirl_arch (+ seasons.toml, docs/SEASONS.md, outfits/halloween.png) |
| feat/seasonal-valentines-spring | e5bbd05 | valentines_strawberry_lick, spring_lollipop_lick (+ outfits/valentines.png) |
| feat/seasonal-summer-newyear | 9a6882b | summer_shades_peek_wink, summer_string_oops (popcheck "specks" = zoom-cropped tail tip, accepted) |

GUI wiring: see docs/SEASONS.md on feat/seasonal-outfits (config key, Seasons loader, Avatar > Outfit menu, companion thumbnails,
`seasonal` tag in catalog::category_of). seasons.toml already lists all 6 seasons; outfits with empty clip lists = unfinished.

## Unfinished (backed up on wip/seasonal-backup — NOT READY)
Raw takes are in `assets/source/video/<clip>/` (s1/s2[/s1b...] mp4, 720p, first=last=peak still); unready renders in
`wip/seasonal/unready/<clip>/` (not in manifest.json). Scripts per worker in `wip/seasonal/scripts/`, kit + recipe in `wip/seasonal/`.

| clip | state | resume from |
| --- | --- | --- |
| xmas_candy_cane_lick | rendered; after fixall+holefill: white-edge FAIL (1 bad frame png+mp4), fringeqa FAIL (pocket 29) | unready render -> fixloop.sh, then hfpost.sh |
| xmas_gift_sit_crossleg | 2 takes chained; zoom_tease killed mid-run | recipe §3 zoom (seated: no --snap) -> proc -> holefill -> QA |
| xmas_mistletoe_blow_kiss | 2 takes chained; zoom_tease killed mid-run | recipe §3 (standing, --snap) |
| xmas_scarf_shiver_hug | 2 takes (s2 near-slip, all 193 frames checked covered) | recipe §3 |
| valentines_kneel_chocolate | rendered; fringeqa FAIL, white-edge FAIL, 36 specks | unready render -> fixloop -> post.sh |
| valentines_heart_squeeze | zoom OOM-killed (rc=137) | recipe §3 |
| valentines_strap_oops, spring_kneel_berries, spring_vine_oops | raw takes only (no chain) | chain (recipe §2) -> verify every oops frame -> §3 |
| spring_petal_squeeze | rendered; white-edge FAIL, 89 specks | unready render -> fixloop -> post.sh |
| summer_popsicle_lick, summer_lotion_squeeze | zoom done/running by SN worker, not processed | proc -> holefill -> QA |
| newyear_champagne_rim_lick, newyear_midnight_kiss_squeeze, newyear_strap_oops, newyear_strawberry_bite | takes chained, no render | recipe §3 |
| secret_oops_retie (feat/secret-oops-pose) | rendered; white-edge FAIL (3 bad frames), 156 specks, loop seam 5 px -> not shipped | unready render -> fixloop -> holefill -> per-frame coverage check -> push |

Every step: holefill after matte-clean/fixall, QA without matte-clean, one heavy matte job at a time, delete intermediates per clip.
import-manifest: no prebuilt binary left after cleanup — rebuild once (recipe §6, gated cargo, CARGO_TARGET_DIR=target-seasonal, -j 2).

## Open issue: tease-holes (main)
All 10 pushed tease_*/squeeze_* clips on main have enclosed matte-clean holes (holefill --dry: 150-190 frames each, up to ~230 px).
holefill + QA (no mc) then FAILED white-edge/fringeqa (e.g. tease_arms_crossed_lift 180/204 bad frames, pockets 32k) — the fills or
the older clips' light rims trip the checks. Nothing committed; local branch fix/tease-holes was removed with its worktree.
Resume: new branch fix/tease-holes off main, holefill, then fixall + holefill again, QA; inspect why png pockets explode before pushing.
