"""Synthetic local native-engine benchmark. No API key or Python inference runtime."""
import json, pathlib, subprocess, sys, time
runner,model=map(pathlib.Path,sys.argv[1:3])
worker=subprocess.Popen([str(runner),'--server',str(model)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True)
samples=[('cold','Good morning. How are you today?','en','es'),('warm','Where is the train station?','en','es'),('warm','Bonjour, je voudrais un café, s’il vous plaît.','fr','es'),('long-rejection','A small tool must never silently discard the last words of a selected paragraph. '*90,'en','es'),('after-rejection','Thank you for your help.','en','es')]
for index,(name,text,source,target)in enumerate(samples,1):
    started=time.perf_counter()
    worker.stdin.write(json.dumps({'id':index,'text':text,'source':source,'target':target})+'\n');worker.stdin.flush()
    response=json.loads(worker.stdout.readline())
    assert response['id']==index
    expected=7 if name=='long-rejection' else 0
    assert response['code']==expected,response
    print(json.dumps({'case':name,'seconds':round(time.perf_counter()-started,3),'code':response['code'],'translation':response.get('translation','')}),flush=True)
try:
    rss=int(subprocess.check_output(['ps','-p',str(worker.pid),'-o','rss='],text=True).strip())
    print(json.dumps({'warmWorkerRssMiB':round(rss/1024,1)}))
except (ValueError,FileNotFoundError):pass
worker.stdin.close();assert worker.wait(timeout=10)==0
