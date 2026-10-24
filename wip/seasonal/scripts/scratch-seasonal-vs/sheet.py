import json,os
from PIL import Image, ImageDraw
W='/workspace/lillith/wt-seasonal-vs/assets/generated'
CL=['tease_arms_crossed_lift','tease_hipsway_chest_press','tease_side_lie_cleavage','tease_shoulder_glance_arch','squeeze_lean_forward_arms','squeeze_hands_press_tease','squeeze_bent_over_cleavage','squeeze_seated_lip_bite','squeeze_kneel_press_wink','squeeze_chest_push_shimmy']
out=Image.new('RGBA',(5*400,2*430),(48,48,52,255)); d=ImageDraw.Draw(out)
for i,c in enumerate(CL):
    x,y=(i%5)*400,(i//5)*430
    j=f'{W}/{c}/{c}.json'
    if not os.path.exists(j): d.text((x+10,y+200),c+'\n(pending)',fill='white'); continue
    z=json.load(open(j))['cleavage_zoom']['zoom_per_frame']; f=max(range(len(z)),key=lambda k:z[k])
    sh=Image.open(f'{W}/{c}/{c}_sheet.png'); out.alpha_composite(sh.crop((f*400,0,f*400+400,400)),(x,y))
    d.text((x+6,y+405),f'{c}  f{f} z{z[f]:.2f}  {len(z)}f',fill='white')
os.makedirs('/workspace/lillith/chat_shots',exist_ok=True); out.convert('RGB').save('/workspace/lillith/chat_shots/sexytease_sheet.png')
