# 反向accept失败记录

任务5298ba91-b7f8-4bf8-a960-5616578afe56，执行前验证PID30696及c076be2 EXE哈希。
原始失败调用如下，变量h并未使用，原命令直接构造头部。

```powershell
$b='http://127.0.0.1:38761'
$s=Invoke-RestMethod "$b/api/v1/session" -Headers @{'Sec-Fetch-Site'='same-origin'}
$id='5298ba91-b7f8-4bf8-a960-5616578afe56'
Invoke-RestMethod "$b/api/v1/transfers/$id/accept" -Method Post -Headers @{'Origin'=$b;'X-LANLink-Token'=$s.token}|Out-Null
```

ErrorActionPreference=Stop。PowerShell进程exit_code=1，错误body原文为：

```json
{ "error": "only incoming offers can be accepted" }
```

该次工具输出没有保存HTTP响应状态码，不能称为现场捕获400。精确提交c076be2的http.rs第36至43行将ApiError映射为StatusCode::BAD_REQUEST，因此从代码推定该响应为400。

随后UTC12:41:15.886读取同一任务为incoming/transferring/offset188743680，PID30696、上限2147483648。state.rs第168至170行仅允许incoming且Offered接受，故该失败与对已开始传输任务再次接受的状态冲突一致，不是本地上传失败或哈希断言失败。该错误体来自Agent，PowerShell只是将非成功响应呈现为失败退出。

未捕获是谁或哪一次请求首次接受该任务，不能认定产品自发接受或确定重复调用来源。当前证据不足以判为产品缺陷，也不把失败调用记为成功。错误后没有重复accept；读取部分进度后按授权强杀并完成恢复验证。见[失败后查询记录](reverse-accept-failure-state.json)。没有为补状态码再发一次accept。
