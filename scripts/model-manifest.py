"""Create a pinned download manifest from a converted official model, without uploading it."""
import argparse,hashlib,json,pathlib
p=argparse.ArgumentParser();p.add_argument('model',type=pathlib.Path);p.add_argument('--base-url',required=True);p.add_argument('--id',default='m2m100-418m-int8-v1');a=p.parse_args()
files=[]
for name in ['config.json','model.bin','shared_vocabulary.json','sentencepiece.bpe.model']:
    path=a.model/name;digest=hashlib.file_digest(path.open('rb'),'sha256').hexdigest()
    files.append({'name':name,'url':a.base_url.rstrip('/')+'/'+name,'sha256':digest,'bytes':path.stat().st_size})
print(json.dumps({'id':a.id,'bytes':sum(f['bytes']for f in files),'files':files},indent=2))
