"""Render dumps from the hobby_probe::controller test: python tools/hobbyhorse_render.py <notmoving.tan> <dump dir> <name,name,...> <out.png>"""
import struct,sys,glob,os,numpy as np
import matplotlib; matplotlib.use('Agg'); import matplotlib.pyplot as plt
def tan(path):
    d=open(path,'rb').read(); I=lambda o: struct.unpack_from('<i',d,o)[0]
    f=I(100); os_=I(104); sc=np.array(struct.unpack_from('<3f',d,f+24)); org=np.array(struct.unpack_from('<3f',d,f+36))
    nv=I(os_+72); ov=I(os_+96)
    return np.frombuffer(d,dtype='<u2',count=nv*4,offset=os_+ov).reshape(-1,4)[:,:3].astype(float)*sc+org
W=tan(sys.argv[1]); frames=sys.argv[2]; names=sys.argv[3].split(','); out=sys.argv[4]
def qrot(q,v):
    x,y,z,w=q; u=np.array([x,y,z]); return v + 2*np.cross(u, np.cross(u,v)+w*v)
views=[('front',lambda p:(p[:,1],p[:,2]),lambda p:p[:,0]),('right side',lambda p:(-p[:,0],p[:,2]),lambda p:-p[:,1]),('3/4',lambda p:(0.7*p[:,1]-0.7*p[:,0],p[:,2]+0.25*(0.7*p[:,0]+0.7*p[:,1])),lambda p:0.7*p[:,0]+0.7*p[:,1])]
fig,axs=plt.subplots(len(views),len(names),figsize=(2.6*len(names),2.9*len(views)))
axs=np.atleast_2d(axs).reshape(len(views),len(names))
for j,n in enumerate(names):
    b=np.fromfile(f'{frames}/{n}.bin',dtype='<f4'); t=b[:3]; q=b[3:7]; A=b[7:].reshape(-1,4)[:,:3]
    P=qrot(q,W)+t
    L=np.array([qrot(q,np.array([x,0.,0.]))+t for x in np.linspace(-24,6,60)])
    print(n,'weapon min z %.1f'%P[:,2].min())
    for i,(vn,proj,depth) in enumerate(views):
        ax=axs[i,j]; pts=np.vstack([A,P,L]); col=np.r_[np.zeros(len(A)),np.ones(len(P)+len(L))]
        dd=depth(pts); o=np.argsort(dd)
        x,y=proj(pts[o]); ax.scatter(x,y,s=0.6,c=np.where(col[o]>0,'#c08000','#2050a0'),alpha=0.55,linewidths=0)
        ax.axhline(0,color='k',lw=0.5); ax.set_aspect('equal'); ax.set_xlim(-55,55); ax.set_ylim(-5,95); ax.set_xticks([]); ax.set_yticks([])
        if i==0: ax.set_title(n.replace('mallet_att_3_prim_','swing'),fontsize=7)
        if j==0: ax.set_ylabel(vn,fontsize=8)
plt.tight_layout(); plt.savefig(out,dpi=110)
