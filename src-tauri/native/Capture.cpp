#define _WIN32_WINNT 0x0A00
#define UNICODE
#include <windows.h>
#include <uiautomation.h>
#include <algorithm>
#include <atomic>
#include <iostream>
#include <mutex>
#include <sstream>
#include <string>
#include <thread>
#include <vector>

std::atomic<bool> enabled(false);
std::mutex exclusionMutex;
std::vector<std::wstring> excluded;
HANDLE wakeEvent=nullptr;
std::atomic<bool> dismissPending(false);
DWORD parentPID=0;
class SelectionEvents final: public IUIAutomationEventHandler, public IUIAutomationFocusChangedEventHandler {
  std::atomic<ULONG> references{1};
public:
 HRESULT STDMETHODCALLTYPE QueryInterface(REFIID id,void** object) override {
  if(id==IID_IUnknown||id==IID_IUIAutomationEventHandler)*object=static_cast<IUIAutomationEventHandler*>(this);
  else if(id==IID_IUIAutomationFocusChangedEventHandler)*object=static_cast<IUIAutomationFocusChangedEventHandler*>(this);
  else{*object=nullptr;return E_NOINTERFACE;}AddRef();return S_OK;
 }
 ULONG STDMETHODCALLTYPE AddRef() override{return ++references;}
 ULONG STDMETHODCALLTYPE Release() override{ULONG count=--references;if(!count)delete this;return count;}
 HRESULT STDMETHODCALLTYPE HandleAutomationEvent(IUIAutomationElement*,EVENTID) override{SetEvent(wakeEvent);return S_OK;}
 HRESULT STDMETHODCALLTYPE HandleFocusChangedEvent(IUIAutomationElement*) override{SetEvent(wakeEvent);return S_OK;}
};
LRESULT CALLBACK mouseEvent(int code,WPARAM event,LPARAM data){
 if(code>=0){auto mouse=reinterpret_cast<MSLLHOOKSTRUCT*>(data);if(event==WM_LBUTTONDOWN||event==WM_RBUTTONDOWN){DWORD pid=0;GetWindowThreadProcessId(WindowFromPoint(mouse->pt),&pid);if(pid!=parentPID)dismissPending=true;SetEvent(wakeEvent);}else if(event==WM_LBUTTONUP)SetEvent(wakeEvent);}
 return CallNextHookEx(nullptr,code,event,data);
}
LRESULT CALLBACK keyEvent(int code,WPARAM event,LPARAM data){
 if(code>=0){auto key=reinterpret_cast<KBDLLHOOKSTRUCT*>(data);if((event==WM_KEYDOWN||event==WM_SYSKEYDOWN)&&key->vkCode==VK_ESCAPE){dismissPending=true;SetEvent(wakeEvent);}else if((event==WM_KEYUP||event==WM_SYSKEYUP)&&key->vkCode>=VK_LEFT&&key->vkCode<=VK_DOWN)SetEvent(wakeEvent);}
 return CallNextHookEx(nullptr,code,event,data);
}
std::string utf8(const std::wstring& input) {
  int n=WideCharToMultiByte(CP_UTF8,0,input.data(),(int)input.size(),nullptr,0,nullptr,nullptr);
  std::string output(n,0);
  WideCharToMultiByte(CP_UTF8,0,input.data(),(int)input.size(),output.data(),n,nullptr,nullptr);
  return output;
}
std::wstring wide(const std::string& input) {
  int n=MultiByteToWideChar(CP_UTF8,0,input.data(),(int)input.size(),nullptr,0);
  std::wstring output(n,0);
  MultiByteToWideChar(CP_UTF8,0,input.data(),(int)input.size(),output.data(),n);
  return output;
}
std::string quoted(const std::string& input) {
  std::string output="\"";
  for (unsigned char c:input) {
    if(c=='"'||c=='\\') { output+='\\'; output+=c; }
    else if(c<32) { char buffer[7];sprintf_s(buffer,"\\u%04x",c);output+=buffer; }
    else output+=c;
  }
  return output+'"';
}
std::wstring appName(DWORD pid) {
  wchar_t buffer[32768];DWORD size=32768;
  HANDLE process=OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,FALSE,pid);
  std::wstring path=L"unknown";
  if(process) {
    if(QueryFullProcessImageName(process,0,buffer,&size)) path=std::wstring(buffer,size);
    CloseHandle(process);
  }
  auto pos=path.find_last_of(L"\\/");
  return pos==std::wstring::npos?path:path.substr(pos+1);
}
bool isExcluded(const std::wstring& name) {
  std::lock_guard<std::mutex> lock(exclusionMutex);
  return std::any_of(excluded.begin(),excluded.end(),[&](const auto& item){return _wcsicmp(item.c_str(),name.c_str())==0;});
}
bool unlockedDesktop() {
  HDESK desktop=OpenInputDesktop(0,FALSE,DESKTOP_READOBJECTS);
  if(!desktop) return false;
  wchar_t name[128];DWORD needed=0;
  bool unlocked=GetUserObjectInformationW(desktop,UOI_NAME,name,sizeof(name),&needed)&&_wcsicmp(name,L"Default")==0;
  CloseDesktop(desktop);
  return unlocked;
}
int main(int argc,char** argv) {
  const DWORD parent=argc>1?std::stoul(argv[1]):0;parentPID=parent;wakeEvent=CreateEventW(nullptr,FALSE,FALSE,nullptr);
  SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
  std::thread([]{
    std::string line;
    while(std::getline(std::cin,line)) {
      if(line=="P") continue;
      std::istringstream stream(line);std::string segment;
      std::getline(stream,segment,'\t');enabled=(segment=="1");SetEvent(wakeEvent);
      std::vector<std::wstring> next;
      while(std::getline(stream,segment,'\t')) if(!segment.empty())next.push_back(wide(segment));
      std::lock_guard<std::mutex> lock(exclusionMutex);excluded=std::move(next);
    }
    std::exit(0); // Parent exited. Do not leave a selection reader behind.
  }).detach();
  CoInitializeEx(nullptr,COINIT_MULTITHREADED);
  IUIAutomation* automation=nullptr;
  if(FAILED(CoCreateInstance(CLSID_CUIAutomation,nullptr,CLSCTX_INPROC_SERVER,IID_PPV_ARGS(&automation))))return 1;
  auto handler=new SelectionEvents();IUIAutomationElement* root=nullptr;
  if(SUCCEEDED(automation->GetRootElement(&root))&&root){automation->AddAutomationEventHandler(UIA_Text_TextSelectionChangedEventId,root,TreeScope_Subtree,nullptr,handler);root->Release();}
  automation->AddFocusChangedEventHandler(nullptr,handler);
  SetWinEventHook(EVENT_SYSTEM_FOREGROUND,EVENT_SYSTEM_FOREGROUND,nullptr,[](HWINEVENTHOOK,DWORD,HWND,LONG,LONG,DWORD,DWORD){SetEvent(wakeEvent);},0,0,WINEVENT_OUTOFCONTEXT);
  SetWindowsHookExW(WH_MOUSE_LL,mouseEvent,GetModuleHandleW(nullptr),0);
  SetWindowsHookExW(WH_KEYBOARD_LL,keyEvent,GetModuleHandleW(nullptr),0);
  std::string last;HWND previous=nullptr;
  for(;;) {
    std::string line="{\"kind\":\"clear\"}";
    HWND foreground=GetForegroundWindow();DWORD pid=0;
    GetWindowThreadProcessId(foreground,&pid);
    POINT pointer;GetCursorPos(&pointer);DWORD pointerPID=0;
    GetWindowThreadProcessId(WindowFromPoint(pointer),&pointerPID);
    if(dismissPending.exchange(false))std::cout<<"{\"kind\":\"dismiss\"}"<<std::endl;
    IUIAutomationElement* element=nullptr;
    const auto name=appName(pid);
    if(enabled&&unlockedDesktop()&&pid!=parent&&!isExcluded(name)&&SUCCEEDED(automation->GetFocusedElement(&element))&&element) {
      BOOL password=TRUE;element->get_CurrentIsPassword(&password);
      if(!password) {
        IUIAutomationTextPattern* pattern=nullptr;
        if(SUCCEEDED(element->GetCurrentPatternAs(UIA_TextPatternId,IID_PPV_ARGS(&pattern)))&&pattern) {
          IUIAutomationTextRangeArray* ranges=nullptr;
          if(SUCCEEDED(pattern->GetSelection(&ranges))&&ranges) {
            int count=0;ranges->get_Length(&count);
            if(count>0) {
              IUIAutomationTextRange* range=nullptr;ranges->GetElement(0,&range);
              if(range) {
                BSTR text=nullptr;range->GetText(12001,&text);
                const std::wstring selected=text?std::wstring(text,SysStringLen(text)):L"";
                if(text)SysFreeString(text);
                if(!selected.empty()&&utf8(selected).size()<=12000) {
                  double x=pointer.x,y=pointer.y;SAFEARRAY* rectangles=nullptr;
                  if(SUCCEEDED(range->GetBoundingRectangles(&rectangles))&&rectangles) {
                    double* data=nullptr;LONG lo=0,hi=-1;
                    SafeArrayGetLBound(rectangles,1,&lo);SafeArrayGetUBound(rectangles,1,&hi);
                    if(hi-lo>=3&&SUCCEEDED(SafeArrayAccessData(rectangles,(void**)&data))) {
                      x=data[0];y=data[1]+data[3];SafeArrayUnaccessData(rectangles);
                    }
                    SafeArrayDestroy(rectangles);
                  }
                  std::ostringstream output;
                  output<<"{\"kind\":\"selection\",\"text\":"<<quoted(utf8(selected))<<",\"app\":"<<quoted(utf8(name))<<",\"x\":"<<x<<",\"y\":"<<y<<"}";
                  line=output.str();
                }
                range->Release();
              }
            }
            ranges->Release();
          }
          pattern->Release();
        }
      }
      element->Release();
    }
    if(foreground!=previous) {std::cout<<"{\"kind\":\"clear\"}"<<std::endl;previous=foreground;}
    if(line!=last) {std::cout<<line<<std::endl;last=line;}
    MsgWaitForMultipleObjects(1,&wakeEvent,FALSE,600,QS_ALLINPUT);
    MSG message;while(PeekMessageW(&message,nullptr,0,0,PM_REMOVE)){TranslateMessage(&message);DispatchMessageW(&message);}
  }
}
