#!/usr/bin/env python3
"""Draft manifest_v2.json for generated_v2 (the live generated/manifest.* is not touched).
- one entry per clip folder; `idle` is an alias of idle_a (same sheet) so every existing `idle` lookup works,
  idle_a / idle_b carry tag `variant_of:idle` (engine patch _engine_patch.diff cycles them at loop ends)
- every clip carries tag `baked_motion` (engine patch damps chest/pigtail/breathing springs for them)
- blend_* entries get blend/from/to/from_frame/to_frame (import-manifest blend fields)
- own-pose events: base_clip = the event itself (their pose bases live in _poses/, not in the manifest)"""
import glob, json, os
import os as _o, sys as _s; _s.path.insert(0, _o.path.dirname(_o.path.abspath(__file__))); import paths
V2 = paths.V2
anims = []
# v1 allure / idle_variant flags (the importer turns them into tags; the app's pose cycle picks these clips by name)
V1 = {a['name']: a for a in json.load(open(os.path.join(paths.V1, 'manifest.json')))['animations']}
# v1 names that are near-duplicates of another v2 clip: same sheet, own name (docs/VIDEO_PIPELINE.md)
# batch 13: alternative takes of a base clip -> tag variant_of:<base>. The engine (lillith-anim Animator)
# only cycles variant_of:<idle base> today; walk/sit variants are tagged for a future picker.
VARIANT_OF = {'walk_sassy_right': 'walk_right', 'walk_skip_right': 'walk_right', 'walk_saunter_right': 'walk_right',
              'walk_sassy_left': 'walk_left', 'walk_skip_left': 'walk_left', 'walk_saunter_left': 'walk_left',
              'sit_bored_chin': 'sit_swing', 'sit_cross_legged_bob': 'sit_swing', 'sit_heel_kick': 'sit_swing', 'sit_stretch_yawn': 'sit_swing',
              'idle_bored_sigh': 'idle', 'idle_nail_check': 'idle', 'idle_tail_fidget': 'idle', 'idle_yawn_stretch': 'idle'}
# batch 15/16: allure poses (pin-up + dom persona). DOM clips also get tag `dom` (dominant/thirsty persona
# banter hooks). REJECTED folders are skipped when building the manifest.
PINUP = {'body_roll_slow', 'kneel_hands_on_thighs',
         'wall_lean_invisible', 'blow_kiss_hip_pop', 'hair_lift_stretch'}
DOM = {'dom_beckon_finger', 'dom_point_down', 'dom_arms_crossed_demand', 'smug_smirk', 'hungry_lip_lick',
       'predator_lean_in', 'stalk_prowl_right', 'stalk_prowl_left', 'tail_wrap_pull', 'kiss_then_come_here',
       'soul_contract_sign', 'kneel_pout_beg', 'hug_screen_possessive',
       'seduce_beckon_finger', 'seduce_lip_bite_glance', 'seduce_lean_to_screen_whisper',
       'seduce_blow_kiss_screen', 'seduce_tap_glass'}
# idle-tease batch 1 (art/idle-tease-1): idle/waiting rotation variants
VARIANT_OF.update({'idle_tease_fan_flustered': 'idle', 'idle_tease_self_hug_tail_swish': 'idle', 'idle_tease_desk_chin_lean': 'idle', 'idle_tease_wrist_tap_pout': 'idle', })
VARIANT_OF.update({'stalk_prowl_right': 'walk_right', 'stalk_prowl_left': 'walk_left'})
# idle rotation picks (lillithTaskCompletionBot / parent, 2026-10-23): short gesture loops played once per pick (variant_play_once)
VARIANT_OF.update({c: 'idle' for c in ('smug_smirk', 'hungry_lip_lick', 'tail_wrap_pull', 'hair_lift_stretch', 'body_roll_slow')})
# arch_back_over_shoulder / bend_forward_wink / slow_turn_spin_tease (+ _long) failed batch 15 review but stay
# registered unchanged (lead, 2026-10-23) until a keep/remove decision; list folder names here to exclude them.
REJECTED: set = set()
ALIASES = {'finger_wag_tease': 'lean_tease', 'hands_behind_head_pose': 'idle_flirt_stretch', 'bounce_giggle': 'idle_flirt_shy_giggle'}
def entry(name, folder, j, extra_tags=()):
    e = {k: j[k] for k in ('frame_count', 'frame_width', 'frame_height', 'fps', 'loop', 'baseline_y', 'anchor', 'scale',
                           'body_anchor_x', 'facing', 'frame_durations_ms', 'canvas_compliant') if j.get(k) is not None}
    e.update(name=name, placeholder=False, source=j.get('source', 'grok-imagine-video'), dir=f'{folder}/',
             sheet=f'{folder}/{folder}_sheet.png', manifest=f'{folder}/{folder}.json',
             preview=f'{folder}/{folder}_preview.gif', frames=f'{folder}/frames/',
             tags=['baked_motion', *extra_tags])
    for k in ('allure', 'idle_variant'):
        if V1.get(name, {}).get(k): e[k] = True
    return e
for d in sorted(glob.glob(f'{V2}/*/')):
    f = os.path.basename(d.rstrip('/'))
    if f.startswith('_') or f in REJECTED or not os.path.exists(f'{d}{f}.json'): continue
    j = json.load(open(f'{d}{f}.json'))
    if f in ('idle_a', 'idle_b'):
        anims.append(entry(f, f, j, ['variant_of:idle']))
        if f == 'idle_a':
            a = entry('idle', f, j); a['alias_of'] = 'idle_a'; anims.append(a)
        continue
    e = entry(f, f, j, [f'variant_of:{VARIANT_OF[f]}'] if f in VARIANT_OF else [])
    if VARIANT_OF.get(f) == 'idle': e['idle_variant'] = True
    if f.startswith('stalk_prowl_'):  # locomotion: dom tag only, not an allure pose
        e['tags'] = sorted(set(e['tags']) | {'dom'})
    elif f in PINUP or f in DOM:
        e['allure'] = True
        e['tags'] = sorted(set(e['tags']) | {'allure'} | ({'dom'} if f in DOM else set()))
    if f.startswith('blend_'):
        sf = j.get('source_frames', {})
        fr, to = j.get('from_clip', 'wake'), j.get('to_clip', 'sleep')
        def ix(c, i):
            n = json.load(open(f'{V2}/{c}/{c}.json'))['frame_count']; return i % n
        ff, tf = ix(fr, sf[fr]), ix(to, sf[to])
        e.update(blend=True, **{'from': 'idle' if fr == 'idle_a' else fr, 'to': 'idle' if to == 'idle_a' else to,
                 'from_frame': ff, 'to_frame': tf})
        e['scale'] = j.get('scale') or 'interpolated between the two clips'
    if f.startswith('event_') and str(j.get('base_clip', '')).startswith('_poses/'):
        e.update(base_clip=f, base_frames=list(range(j['frame_count'])), pose_base_dir=j['base_clip'] + '/')
    anims.append(e)
for al, tgt in ALIASES.items():
    j = json.load(open(f'{V2}/{tgt}/{tgt}.json')); a = entry(al, tgt, j); a['alias_of'] = tgt; anims.append(a)
anims.sort(key=lambda e: e['name'])
m = {'style': 'generated_v2: video-based (Grok Imagine I2V) with baked bounce/secondary motion, canonical_ko look',
     'canvas': {'w': 400, 'h': 400, 'baseline_y': 392, 'anchor': [200, 392]},
     'jiggle_note': 'All clips are tagged baked_motion: the engine must damp chest/pigtail springs and breathing for them (see _engine_patch.diff).',
     'notes': 'DRAFT, not wired into the app. idle = alias of idle_a; idle_a/idle_b are variant_of:idle for random cycling; idle_flirt_* keep idle_variant (pose-cycle mannerisms, not idle loops); finger_wag_tease, hands_behind_head_pose and bounce_giggle are aliases of near-duplicate clips.',
     'animations': anims}
json.dump(m, open(f'{V2}/manifest_v2.json', 'w'), indent=1)
print(len(anims), 'entries')
