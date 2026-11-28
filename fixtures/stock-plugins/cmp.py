import json,sys
a=json.load(open(sys.argv[1]));b=json.load(open(sys.argv[2]))
def d(x,y,p=""):
    if type(x)!=type(y): yield p; return
    if isinstance(x,dict):
        for k in set(x)|set(y):
            if k not in x or k not in y: yield p+"/"+k
            else: yield from d(x[k],y[k],p+"/"+k)
    elif isinstance(x,list):
        if len(x)!=len(y): yield p+"[len]"
        else:
            for i,(u,v) in enumerate(zip(x,y)): yield from d(u,v,p+"/"+str(i))
    elif x!=y: yield p
n=0
for s,t in zip(a['steps'],b['steps']):
    for p in d(s,t): print(s['name'],p); n+=1
print("diffs",n)
