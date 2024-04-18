// Author: Ashley Wulber
// Title: Rounded rectangle
#extension GL_OES_standard_derivatives:enable
#ifdef GL_ES
precision mediump float;
#endif

// canvas
uniform vec2 size;

uniform float rad_tl;
uniform float rad_tr;
uniform float rad_bl;
uniform float rad_br;
uniform vec2 loc;
uniform vec2 rect_size;

uniform float border_width;
uniform float drop_shadow;
uniform vec4 bg_color;
uniform vec4 border_color;

float sdRoundBox(in vec2 p,in vec2 b,in vec4 r)
{
    r.xy=(p.x>0.)?r.xy:r.zw;
    r.x=(p.y>0.)?r.x:r.y;
    vec2 q=abs(p)-b+r.x;
    return min(max(q.x,q.y),0.)+length(max(q,0.))-r.x;
}

void main()
{
    vec2 p=2.*gl_FragCoord.xy-(rect_size+loc*2.);
    
    vec2 si=rect_size;
    vec4 ra=2.*vec4(rad_tr,rad_br,rad_tl,rad_bl);
    ra=min(ra,min(si.x,si.y));
    
    float d=sdRoundBox(p,si,ra);
    
    vec2 tl_corner=vec2(loc.x,loc.y+rect_size.y);
    vec2 tr_corner=vec2(loc.x+rect_size.x,loc.y+rect_size.y);
    vec2 bl_corner=vec2(loc.x,loc.y);
    vec2 br_corner=vec2(loc.x+rect_size.x,loc.y);
    vec2 pos=gl_FragCoord.xy;
    
    float delta=0.;
    float d_tl=distance(pos,tl_corner);
    float d_tr=distance(pos,tr_corner);
    float d_bl=distance(pos,bl_corner);
    float d_br=distance(pos,br_corner);
    
    if(d_tl<ra.z){
        delta=(ra.z-d_tl)/ra.z*fwidth(d)*2.;
    }else if(d_tr<ra.x){
        delta=(ra.x-d_tr)/ra.x*fwidth(d)*2.;
    }else if(d_bl<ra.w){
        delta=(ra.w-d_bl)/ra.w*fwidth(d)*2.;
    }else if(d_br<ra.y){
        delta=(ra.y-d_br)/ra.y*fwidth(d)*2.;
    }
    vec4 calc_bg_color;
    
    vec2 p_border=vec2(rect_size.x-border_width*2.,0.);
    float d_border=sdRoundBox(p_border,si,ra);
    
    vec2 p_drop_shadow=vec2(rect_size.x+drop_shadow*2.,0.);
    float d_drop_shadow=sdRoundBox(p_drop_shadow,si,ra);
    
    if(d>d_border&&d<=0.){
        calc_bg_color=border_color;
        calc_bg_color*=1.-smoothstep(1.-delta/2.,1.+delta/2.,1.+d);
        
    }else if(d>0.&&d<d_drop_shadow){
        calc_bg_color=vec4(.1,.1,.1,.7);
        calc_bg_color*=1.-smoothstep(0.,1.,d/d_drop_shadow);
    }else{
        calc_bg_color=bg_color;
        calc_bg_color*=1.-smoothstep(1.-3.*delta/4.,1.+delta/4.,1.+d);
    }
    
    // now we need to adjust the rect_size to account for the border width
    // and then blend the border color with the background color
    
    // TODO: add border width
    
    // lastly, add the drop shadow
    // this expands the rect_size to account for the drop shadow
    // and then blends the shadow color with the previously determined color
    // for this pixel
    
    // TDOD: add drop shadow
    
    gl_FragColor=calc_bg_color;
}

