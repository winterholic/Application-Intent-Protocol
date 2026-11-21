import {post,show} from './raw.mjs';
let r;
r=await post('/read',{query:{read:'Product',select:['id','sku','stock','price']}}); show('read Product',r);
// 1. bundle: create order + line with refs + take input
r=await post('/apply',{atomic:[
 {create:'Order',values:{owner:'42',total:'20.00',status:'open'}},
 {create:'OrderLine',values:{product:'1',qty:3,amount:'15.00'},refs:{order:{created:0}}},
 {apply:'Product.take',target:{ids:['1']},input:{qty:3}}]},{key:'b1'}); show('bundle refs+input',r);
r=await post('/apply',{atomic:[
 {create:'Order',values:{owner:'42',total:'20.00',status:'open'}},
 {create:'OrderLine',values:{product:'1',qty:3,amount:'15.00'},refs:{order:{created:0}}},
 {apply:'Product.take',target:{ids:['1']},input:{qty:3}}]},{key:'b1'}); show('replay',r);
// 2. target.created
r=await post('/apply',{atomic:[
 {create:'Order',values:{owner:'42',total:'1.00',status:'open'}},
 {apply:'Order.confirm',target:{created:0}}]},{key:'b2'}); show('bundle target.created',r);
// 3. compose in bundle
r=await post('/read',{query:{read:'Order',select:['id','total','status']}}); show('read Order',r);
show('read Order lineSum',await post('/read',{query:{read:'Order',select:['id','lineSum']}}));
show('read Order lineCount',await post('/read',{query:{read:'Order',select:['id','lineCount']}}));
const ids=r.body.rows.map(x=>x.id);
r=await post('/apply',{atomic:[
 {compose:'Order',targets:{ids:[ids[0]]},steps:[{transition:'confirm'},{create:'Note',values:{owner:{item:'owner'}}}]},
 {apply:'Product.take',target:{ids:['2']},input:{qty:1}}]},{key:'b3'}); show('bundle compose',r);
r=await post('/apply',{atomic:[
 {create:'Note',values:{owner:'42',text:'hi'}}]},{key:'b4'}); show('bundle plain (no ops)',r);
// 4. standalone compose
r=await post('/apply',{compose:'Order',targets:{ids:[ids[1]]},steps:[{transition:'confirm'}]},{key:'b5'}); show('compose standalone',r);
// 5. Decimal input
r=await post('/apply',{apply:'Product.restock',target:{ids:['1']},input:{amount:'1.25'}},{key:'d1'}); show('restock 1.25',r);
r=await post('/apply',{apply:'Product.restock',target:{ids:['1']},input:{amount:'0.00'}},{key:'d2'}); show('restock 0.00',r);
r=await post('/apply',{apply:'Product.restock',target:{ids:['1']},input:{amount:1.25}},{key:'d3'}); show('restock number 1.25',r);
r=await post('/apply',{apply:'Product.restock',target:{ids:['1']},input:{amount:'1.255'}},{key:'d4'}); show('restock 1.255',r);
r=await post('/apply',{apply:'Product.restock',target:{ids:['1']},input:{amount:'-1.00'}},{key:'d5'}); show('restock -1.00',r);
r=await post('/apply',{apply:'Product.take',target:{ids:['1']},input:{qty:0}},{key:'i1'}); show('take 0',r);
r=await post('/apply',{apply:'Product.take',target:{ids:['1']},input:{qty:1001}},{key:'i2'}); show('take 1001',r);
r=await post('/apply',{apply:'Product.take',target:{ids:['1']},input:{}},{key:'i3'}); show('take {} missing key',r);
r=await post('/apply',{apply:'Product.take',target:{ids:['1']}},{key:'i4'}); show('take no input',r);
r=await post('/apply',{apply:'Product.take',target:{ids:['1']},input:{qty:1,zzz:1}},{key:'i5'}); show('take unknown input key',r);
r=await post('/apply',{apply:'Order.confirm',target:{ids:[ids[0]]},input:{qty:1}},{key:'i6'}); show('input on non-input transition',r);
r=await post('/apply',{apply:'Product.take',target:{ids:['1']},input:{qty:'3'}},{key:'i7'}); show('take qty string',r);
r=await post('/apply',{apply:'Product.take',target:{ids:['1']},input:{qty:1.5}},{key:'i8'}); show('take qty 1.5',r);
r=await post('/apply',{apply:'Product.take',target:{ids:['1']},input:{qty:99999}},{key:'i9'}); show('take over stock',r);
